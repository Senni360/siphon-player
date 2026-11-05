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

/// The progress that [`ease`] turned into `eased`, for what must travel evenly,
/// or in a shape of its own, on a transition's clock.
pub fn unease(eased: f32) -> f32 {
    1.0 - (1.0 - eased.clamp(0.0, 1.0)).powf(0.2)
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

/// Marks this window as one that comes in when the app starts. Views drawn alone,
/// as in tests, are simply there.
pub fn welcome(ctx: &Context) {
    ctx.data_mut(|data| data.insert_temp(Id::new("kit-welcome"), true));
}

/// Eased progress of something coming in when the app starts, over `duration`
/// after waiting `delay`. It waits for a frame or two to be drawn first, as the
/// first frame is a slow one and would eat the motion.
pub fn arrival(ctx: &Context, id: Id, duration: f32, delay: f32) -> f32 {
    if !ctx.data(|data| {
        data.get_temp::<bool>(Id::new("kit-welcome"))
            .unwrap_or(false)
    }) {
        return 1.0;
    }
    if ctx.cumulative_frame_nr() < 2 {
        ctx.request_repaint();
        return 0.0;
    }
    appear(ctx, id, duration, delay)
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

/// Small bars dancing in `rect`: this is the song that is playing. They only ask
/// for a few frames a second, and rest when less motion is asked for.
pub fn equalizer(ui: &egui::Ui, rect: egui::Rect, color: egui::Color32) {
    const BARS: [(f32, f32); 4] = [(9.1, 0.0), (6.7, 1.7), (11.3, 3.1), (7.9, 4.6)];
    let moving = crate::motion::seconds(ui.ctx(), 1.0) > 0.0;
    let clock = if moving {
        ui.input(|input| input.time) as f32
    } else {
        0.6
    };
    let bar = rect.width() * 0.16;
    let gap = rect.width() * 0.09;
    let total = bar * BARS.len() as f32 + gap * (BARS.len() as f32 - 1.0);
    let mut x = rect.center().x - total / 2.0;
    for (speed, phase) in BARS {
        let swing = ((clock * speed + phase).sin() * 0.5 + 0.5) * 0.65 + 0.35;
        let height = rect.height() * 0.8 * swing;
        let shape = egui::Rect::from_min_max(
            egui::pos2(x, rect.center().y + rect.height() * 0.4 - height),
            egui::pos2(x + bar, rect.center().y + rect.height() * 0.4),
        );
        ui.painter().rect_filled(shape, bar / 2.0, color);
        x += bar + gap;
    }
    if moving {
        crate::theme::repaint_after(ui.ctx(), std::time::Duration::from_millis(66));
    }
}

/// `text` with each of its numbers counted up to `t` of its value, keeping its
/// thousands separators, for figures that tick up as they come into view.
pub fn counting(text: &str, t: f32) -> String {
    if t >= 1.0 {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.is_empty() {
            return;
        }
        let spaced = run.contains(',');
        let digits: String = run.chars().filter(char::is_ascii_digit).collect();
        let value = digits.parse::<f64>().unwrap_or(0.0) * f64::from(t.max(0.0));
        let shown = (value as u64).to_string();
        if spaced {
            for (index, digit) in shown.chars().enumerate() {
                if index > 0 && (shown.len() - index).is_multiple_of(3) {
                    out.push(',');
                }
                out.push(digit);
            }
        } else {
            out.push_str(&shown);
        }
        run.clear();
    };
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let inside = c == ',' && !run.is_empty() && chars.peek().is_some_and(char::is_ascii_digit);
        if c.is_ascii_digit() || inside {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// A ring opening out of `center` as `t` goes from 0 to 1.
pub fn ring(
    painter: &egui::Painter,
    center: egui::Pos2,
    from: f32,
    to: f32,
    t: f32,
    color: egui::Color32,
) {
    if t <= 0.0 || t >= 1.0 {
        return;
    }
    painter.circle_stroke(
        center,
        from + (to - from) * t,
        egui::Stroke::new(2.0 * (1.0 - t), color.gamma_multiply(1.0 - t)),
    );
}

/// A soft, slow field of colour behind a view about the playing song. The blobs
/// drift by `clock`, which only runs while the song plays; `swell` (0 to 1) lets
/// them breathe with the sound.
pub fn aurora(
    painter: &egui::Painter,
    rect: egui::Rect,
    colors: &[egui::Color32],
    clock: f32,
    swell: f32,
    strength: f32,
) {
    const PATHS: [(f32, f32, f32, f32); 4] = [
        (0.21, 0.13, 0.0, 1.3),
        (0.13, 0.17, 2.1, 0.4),
        (0.17, 0.11, 4.0, 2.7),
        (0.09, 0.23, 5.2, 3.9),
    ];
    if colors.is_empty() || rect.width() <= 0.0 {
        return;
    }
    let reach = rect.size().max_elem() * (0.46 + 0.05 * swell);
    for (index, (speed_x, speed_y, phase_x, phase_y)) in PATHS.iter().enumerate() {
        let x = 0.5 + 0.42 * (clock * speed_x + phase_x).sin();
        let y = 0.55 + 0.42 * (clock * speed_y + phase_y).cos();
        let center = egui::pos2(
            rect.left() + rect.width() * x,
            rect.top() + rect.height() * y,
        );
        let color = colors[index % colors.len()].gamma_multiply(strength);
        spotlight(painter, rect, center, reach, color);
    }
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

/// How far long text has slid to show its end, while `active`: it waits a moment,
/// reads along at an even pace, rests at the end, and comes back. When `overflow`
/// is nothing, nothing moves. Asks for frames only while it moves.
pub fn glide_along(ctx: &Context, id: Id, overflow: f32, active: bool) -> f32 {
    const WAIT: f64 = 0.5;
    const PACE: f64 = 40.0;
    if overflow <= 1.0 || crate::motion::seconds(ctx, 1.0) <= 0.0 {
        return 0.0;
    }
    let now = ctx.input(|input| input.time);
    let started_id = id.with("started");
    let target = if active {
        let started = ctx.data_mut(|data| *data.get_temp_mut_or_insert_with(started_id, || now));
        let travel = f64::from(overflow) / PACE;
        let elapsed = (now - started) - WAIT;
        let cycle = (travel + WAIT) * 2.0;
        let place = elapsed.rem_euclid(cycle);
        if elapsed < 0.0 {
            0.0
        } else if place < travel {
            ctx.request_repaint();
            (place * PACE) as f32
        } else if place < travel + WAIT {
            ctx.request_repaint();
            overflow
        } else if place < travel * 2.0 + WAIT {
            ctx.request_repaint();
            overflow - ((place - travel - WAIT) * PACE) as f32
        } else {
            ctx.request_repaint();
            0.0
        }
    } else {
        ctx.data_mut(|data| data.remove::<f64>(started_id));
        0.0
    };
    if active {
        ctx.request_repaint();
        spring_jump(ctx, id, target);
        target
    } else {
        spring(ctx, id, target, FOLLOW)
    }
}

/// How a [`spring`] moves: how long one swing takes, in seconds, and how much
/// it overshoots, from 0 (settles without passing) to about 0.5 (a lively bounce).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub period: f32,
    pub bounce: f32,
}

/// Follows without overshooting, for what the pointer drags along.
pub const FOLLOW: Spring = Spring {
    period: 0.22,
    bounce: 0.0,
};
/// A small lively bounce, for what answers a hover or a press.
pub const LIVELY: Spring = Spring {
    period: 0.26,
    bounce: 0.35,
};

/// One step of a spring from `position` and `velocity` toward `target`.
fn spring_step(position: f32, velocity: f32, target: f32, spring: Spring, dt: f32) -> (f32, f32) {
    let omega = std::f32::consts::TAU / spring.period.max(0.01);
    let damping = 2.0 * (1.0 - spring.bounce).clamp(0.05, 1.0) * omega;
    let (mut position, mut velocity) = (position, velocity);
    // Short steps keep a slow frame from throwing it past where it should be.
    let steps = (dt / (1.0 / 240.0)).ceil().clamp(1.0, 16.0);
    let dt = dt / steps;
    for _ in 0..steps as usize {
        velocity += (omega * omega * (target - position) - damping * velocity) * dt;
        position += velocity * dt;
    }
    (position, velocity)
}

/// A number that follows `target` as if on a spring, keeping its speed when the
/// target changes mid-way, so an interrupted motion never jumps. It starts at the
/// first target, asks for frames only while it moves, and goes straight to the
/// target when less motion is asked for.
pub fn spring(ctx: &Context, id: Id, target: f32, spring: Spring) -> f32 {
    if crate::motion::seconds(ctx, 1.0) <= 0.0 {
        return target;
    }
    let (position, velocity) = ctx
        .data(|data| data.get_temp::<(f32, f32)>(id))
        .unwrap_or((target, 0.0));
    if position == target && velocity == 0.0 {
        ctx.data_mut(|data| data.insert_temp(id, (position, velocity)));
        return target;
    }
    let dt = ctx
        .input(|input| input.stable_dt)
        .clamp(1.0 / 480.0, 1.0 / 20.0);
    let (mut position, mut velocity) = spring_step(position, velocity, target, spring, dt);
    let scale = (target.abs() + 1.0) * 0.0005;
    if (position - target).abs() < scale && velocity.abs() < scale * 20.0 {
        position = target;
        velocity = 0.0;
    } else {
        ctx.request_repaint();
    }
    ctx.data_mut(|data| data.insert_temp(id, (position, velocity)));
    position
}

/// Sets where a [`spring`] is, with no speed, for something that appears
/// somewhere new rather than travelling there.
pub fn spring_jump(ctx: &Context, id: Id, position: f32) {
    ctx.data_mut(|data| data.insert_temp(id, (position, 0.0_f32)));
}

/// How strong a hover is on its own: the pointer's position when it is within
/// `rect`, and how far the hover has eased in.
pub fn pointer_light(ui: &egui::Ui, id: Id, rect: egui::Rect) -> Option<(egui::Pos2, f32)> {
    let pointer = ui.input(|input| input.pointer.hover_pos());
    let inside = pointer
        .filter(|pos| rect.contains(*pos) && ui.clip_rect().contains(*pos) && ui.is_enabled());
    let lit = toward(ui.ctx(), id, inside.is_some(), FEEDBACK * 1.5);
    if lit <= 0.0 {
        return None;
    }
    if let Some(pos) = inside {
        ui.ctx()
            .data_mut(|data| data.insert_temp(id.with("at"), pos));
    }
    let pos = ui
        .ctx()
        .data(|data| data.get_temp::<egui::Pos2>(id.with("at")))?;
    Some((pos, lit))
}

/// A soft round light at `center`, fading out over `radius`, kept inside `rect`.
pub fn spotlight(
    painter: &egui::Painter,
    rect: egui::Rect,
    center: egui::Pos2,
    radius: f32,
    color: egui::Color32,
) {
    const SEGMENTS: u32 = 28;
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, color);
    // The light falls off in two steps, which reads as a curve and not a cone.
    let middle = color.gamma_multiply(0.38);
    for ring in [0.5_f32, 1.0] {
        for segment in 0..SEGMENTS {
            let angle = segment as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let shade = if ring < 1.0 {
                middle
            } else {
                egui::Color32::TRANSPARENT
            };
            mesh.colored_vertex(center + egui::Vec2::angled(angle) * radius * ring, shade);
        }
    }
    for segment in 0..SEGMENTS {
        let next = (segment + 1) % SEGMENTS;
        mesh.add_triangle(0, 1 + segment, 1 + next);
        let (a, b) = (1 + segment, 1 + next);
        let (c, d) = (1 + SEGMENTS + segment, 1 + SEGMENTS + next);
        mesh.add_triangle(a, c, d);
        mesh.add_triangle(a, d, b);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// The outline of a rounded rectangle as points, clockwise from its top left.
fn rounded_outline(rect: egui::Rect, radius: f32) -> Vec<egui::Pos2> {
    use std::f32::consts::{FRAC_PI_2, PI};
    let radius = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let corners = [
        (egui::pos2(rect.left() + radius, rect.top() + radius), PI),
        (
            egui::pos2(rect.right() - radius, rect.top() + radius),
            PI + FRAC_PI_2,
        ),
        (
            egui::pos2(rect.right() - radius, rect.bottom() - radius),
            0.0,
        ),
        (
            egui::pos2(rect.left() + radius, rect.bottom() - radius),
            FRAC_PI_2,
        ),
    ];
    let mut points = Vec::with_capacity(64);
    let mut last: Option<egui::Pos2> = None;
    for (center, start) in corners {
        for step in 0..=5 {
            let angle = start + step as f32 / 5.0 * FRAC_PI_2;
            let point = center + egui::Vec2::angled(angle) * radius;
            // Long edges get points along them, so a light can fade along them.
            if step == 0
                && let Some(last) = last
            {
                let pieces = ((point - last).length() / 16.0).ceil().max(1.0) as usize;
                for piece in 1..pieces {
                    points.push(last + (point - last) * (piece as f32 / pieces as f32));
                }
            }
            points.push(point);
            last = Some(point);
        }
    }
    if let (Some(&first), Some(&end)) = (points.first(), points.last()) {
        let pieces = ((first - end).length() / 16.0).ceil().max(1.0) as usize;
        for piece in 1..pieces {
            points.push(end + (first - end) * (piece as f32 / pieces as f32));
        }
    }
    points
}

/// Paints a card's edge with a strength for each point along it.
fn paint_edge(
    painter: &egui::Painter,
    points: &[egui::Pos2],
    width: f32,
    color: impl Fn(usize, egui::Pos2) -> egui::Color32,
) {
    let count = points.len() as u32;
    if count < 3 {
        return;
    }
    let center = points
        .iter()
        .fold(egui::Vec2::ZERO, |sum, point| sum + point.to_vec2())
        / count as f32;
    let mut mesh = egui::Mesh::default();
    for (index, point) in points.iter().enumerate() {
        let inward = (center - point.to_vec2()).normalized();
        let shade = color(index, *point);
        mesh.colored_vertex(*point, shade);
        mesh.colored_vertex(*point + inward * width, shade);
    }
    for index in 0..count {
        let next = (index + 1) % count;
        mesh.add_triangle(index * 2, index * 2 + 1, next * 2 + 1);
        mesh.add_triangle(index * 2, next * 2 + 1, next * 2);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// The stretch of a card's edge nearest the pointer catches the light.
pub fn edge_light(
    painter: &egui::Painter,
    rect: egui::Rect,
    radius: f32,
    pointer: egui::Pos2,
    reach: f32,
    color: egui::Color32,
) {
    let points = rounded_outline(rect, radius);
    paint_edge(painter, &points, 1.2, |_, point| {
        let near = (1.0 - (point - pointer).length() / reach).clamp(0.0, 1.0);
        color.gamma_multiply(near * near)
    });
}

/// How long a light takes to run around a card's edge.
pub const EDGE_RUN: f32 = 0.9;

/// A light running once around a card's edge in both directions from its
/// bottom left, meeting at the far side, as `t` goes from 0 to 1.
pub fn edge_run(
    painter: &egui::Painter,
    rect: egui::Rect,
    radius: f32,
    t: f32,
    color: egui::Color32,
) {
    if t <= 0.0 || t >= 1.0 {
        return;
    }
    let points = rounded_outline(rect, radius);
    let count = points.len() as f32;
    // The outline starts at the top left: this is near the song's cover.
    let start = count * 0.82;
    let head = t * 0.5;
    let fade = (1.0 - t) * (t * 8.0).min(1.0);
    paint_edge(painter, &points, 1.6, |index, _| {
        let along = ((index as f32 - start) / count).rem_euclid(1.0);
        let along = along.min(1.0 - along);
        // A bright head with a tail reaching back to where it set out.
        let behind = head - along;
        if behind < 0.0 {
            return egui::Color32::TRANSPARENT;
        }
        let strength = (1.0 - behind / 0.28).clamp(0.0, 1.0);
        color.gamma_multiply(strength * strength * fade)
    });
}

/// How long a press's ripple takes to spread.
pub const RIPPLE: f32 = 0.5;

/// A ripple from where `response` was pressed, spreading across `rect`.
pub fn ripple(ui: &egui::Ui, response: &egui::Response, rect: egui::Rect, color: egui::Color32) {
    let id = response.id.with("ripple");
    if (response.clicked() || response.secondary_clicked())
        && let Some(pos) = response.interact_pointer_pos()
    {
        ui.ctx()
            .data_mut(|data| data.insert_temp(id.with("at"), pos));
        begin(ui.ctx(), id, RIPPLE);
    }
    let Some(t) = running(ui.ctx(), id) else {
        return;
    };
    let Some(at) = ui
        .ctx()
        .data(|data| data.get_temp::<egui::Pos2>(id.with("at")))
    else {
        return;
    };
    let reach = (at - rect.center()).length() + rect.size().length() / 2.0;
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.circle_filled(at, reach * t, color.gamma_multiply(0.14 * (1.0 - t)));
}

/// How long the light takes to cross a button that is pointed at.
pub const GLINT: f32 = 0.6;

/// A slanted light passing once across `rect` as `t` goes from 0 to 1.
pub fn glint(painter: &egui::Painter, rect: egui::Rect, t: f32, color: egui::Color32) {
    if t <= 0.0 || t >= 1.0 {
        return;
    }
    let width = rect.height() * 1.4;
    let slant = rect.height() * 0.5;
    let left = rect.left() - width - slant + (rect.width() + width + slant * 2.0) * t;
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let mut mesh = egui::Mesh::default();
    let clear = egui::Color32::TRANSPARENT;
    for (offset, shade) in [(0.0, clear), (0.5, color), (1.0, clear)] {
        let x = left + width * offset;
        mesh.colored_vertex(egui::pos2(x + slant, rect.top()), shade);
        mesh.colored_vertex(egui::pos2(x, rect.bottom()), shade);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    mesh.add_triangle(2, 3, 4);
    mesh.add_triangle(3, 4, 5);
    painter.add(egui::Shape::mesh(mesh));
}

/// Starts a [`GLINT`] whenever the pointer arrives on something, and returns
/// its progress while it runs.
pub fn glint_on_hover(ctx: &Context, id: Id, hovered: bool) -> Option<f32> {
    let was = ctx
        .data(|data| data.get_temp::<bool>(id.with("was")))
        .unwrap_or(false);
    if hovered && !was {
        begin(ctx, id.with("glint"), GLINT);
    }
    ctx.data_mut(|data| data.insert_temp(id.with("was"), hovered));
    running(ctx, id.with("glint"))
}

/// How an icon answers being pointed at and pressed. Each says what its control
/// does: a gear turns, a chevron leans the way it goes, a download dips.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gesture {
    /// A turn while pointed at, in degrees, and a whole gesture on a press.
    pub hover_turn: f32,
    pub press_turn: f32,
    /// A lean while pointed at, in points.
    pub hover_move: (f32, f32),
    /// A shake from side to side on a press.
    pub press_shake: f32,
    /// A dip downward and back on a press, in points.
    pub press_dip: f32,
}

/// How long a pressed icon's gesture takes.
pub const GESTURE: f32 = 0.5;

/// Where an icon sits at `hover` (0 to 1, and past 1 on a bounce) and the
/// `press` gesture's raw progress: its offset and its turn in radians.
pub fn gesture_pose(gesture: Gesture, hover: f32, press: Option<f32>) -> (egui::Vec2, f32) {
    let mut offset = egui::vec2(gesture.hover_move.0, gesture.hover_move.1) * hover;
    let mut turn = gesture.hover_turn * hover;
    if let Some(t) = press {
        let t = t.clamp(0.0, 1.0);
        turn += gesture.press_turn * ease(t);
        offset.x += gesture.press_shake * (1.0 - t) * (t * std::f32::consts::PI * 5.0).sin();
        offset.y += gesture.press_dip * (t * std::f32::consts::PI).sin();
    }
    (offset, turn.to_radians())
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

    /// Counting changes only the numbers, and ends at the words it was given.
    #[test]
    fn numbers_count_up_and_the_words_stay() {
        assert_eq!(counting("12,346 saves", 1.0), "12,346 saves");
        assert_eq!(counting("12,346 saves", 0.5), "6,173 saves");
        assert_eq!(counting("3 hr 13 min", 0.0), "0 hr 0 min");
        assert_eq!(counting("no figures, here", 0.5), "no figures, here");
    }

    #[test]
    fn unease_undoes_the_curve() {
        for step in 0..=10 {
            let t = step as f32 / 10.0;
            assert!((unease(ease(t)) - t).abs() < 1e-3, "{t}");
        }
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

    /// A spring reaches its target from either side and comes to rest there,
    /// a bouncy one passing it on the way and a following one never.
    #[test]
    fn a_spring_settles_on_its_target() {
        for (spring, passes) in [(LIVELY, true), (FOLLOW, false)] {
            let (mut position, mut velocity) = (0.0, 0.0);
            let mut furthest = 0.0_f32;
            for _ in 0..240 {
                (position, velocity) = spring_step(position, velocity, 1.0, spring, 1.0 / 120.0);
                furthest = furthest.max(position);
            }
            assert!(
                (position - 1.0).abs() < 0.001,
                "{spring:?} ended at {position}"
            );
            assert_eq!(furthest > 1.001, passes, "{spring:?} reached {furthest}");
        }
    }

    /// A slow frame cannot throw a spring off: it stays near its range.
    #[test]
    fn a_slow_frame_does_not_throw_a_spring() {
        let (position, _) = spring_step(0.0, 0.0, 1.0, LIVELY, 1.0 / 20.0);
        assert!((0.0..1.5).contains(&position), "{position}");
    }

    #[test]
    fn with_less_motion_a_spring_is_at_its_target() {
        let ctx = egui::Context::default();
        crate::motion::apply(&ctx, true);
        let id = Id::new("spring-test");
        assert_eq!(spring(&ctx, id, 1.0, LIVELY), 1.0);
        assert_eq!(spring(&ctx, id, 5.0, LIVELY), 5.0);
    }

    /// A card's outline runs around the whole card, on its edge.
    #[test]
    fn a_rounded_outline_stays_on_the_card() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(300.0, 80.0));
        let points = rounded_outline(rect, 12.0);
        assert!(points.len() > 24);
        for point in &points {
            assert!(rect.expand(0.01).contains(*point), "{point:?}");
        }
        for pair in points.windows(2) {
            assert!((pair[1] - pair[0]).length() < 17.0, "{pair:?}");
        }
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
