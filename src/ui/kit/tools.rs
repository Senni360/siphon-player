//! The four square tools every song row carries, always in this order:
//! Download, BPM and key, Credits, and Add to the last-used playlist.
//!
//! A tool's tooltip says what it will do or why it cannot, so a disabled
//! tool still answers when hovered. The row reports which tool was used
//! and whether by right-click (Download's options, another playlist);
//! the view turns that into an action.

use egui::{CornerRadius, Pos2, Response, Sense, Stroke, Ui, Vec2, vec2};

use crate::theme::{self, Icon, Palette};

/// A tool's square.
pub const SIZE: f32 = 30.0;
pub const ICON: f32 = 16.0;
pub const GAP: f32 = 2.0;
/// The whole group, for views that reserve its column.
pub const ROW_WIDTH: f32 = SIZE * 4.0 + GAP * 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Download,
    BpmKey,
    Credits,
    AddToLast,
}

impl Tool {
    pub const ALL: [Self; 4] = [Self::Download, Self::BpmKey, Self::Credits, Self::AddToLast];

    pub fn icon(self) -> Icon {
        match self {
            Self::Download => Icon::Download,
            Self::BpmKey => Icon::Activity,
            Self::Credits => Icon::FileText,
            Self::AddToLast => Icon::ListPlus,
        }
    }
}

/// What a tool can do now, with the words its tooltip shows.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Usable: "Download WAV + stems. Right-click for options".
    Ready(String),
    /// Not usable, and why: "Download needs the Siphon helper".
    Off(String),
    /// Work in progress the user can stop, with its share done if known.
    Running { fraction: Option<f32>, tip: String },
    /// Finished a moment ago: "Saved", "Copied".
    Done(String),
}

impl State {
    pub fn tip(&self) -> &str {
        match self {
            Self::Ready(tip) | Self::Off(tip) | Self::Done(tip) => tip,
            Self::Running { tip, .. } => tip,
        }
    }

    pub fn usable(&self) -> bool {
        !matches!(self, Self::Off(_))
    }
}

/// A tool used on a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    pub tool: Tool,
    /// Right-click: Download's options or another playlist.
    pub secondary: bool,
}

/// One tool's square. `reveal` runs from 0, hidden, to 1, in place: rows
/// show their tools on hover, on focus, on the playing row and while
/// something runs, and they slide in a few points as they appear.
pub fn button(ui: &mut Ui, palette: &Palette, tool: Tool, state: &State, reveal: f32) -> Response {
    let sense = if state.usable() {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SIZE), sense);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, state.usable(), state.tip())
    });
    if reveal > 0.0 && ui.is_rect_visible(rect) {
        let rect = rect.translate(vec2((1.0 - reveal) * 6.0, 0.0));
        let painter = ui.painter();
        let lit = state.usable() && (response.hovered() || response.has_focus());
        if lit {
            painter.rect_filled(
                rect,
                CornerRadius::same(theme::RADIUS),
                palette.surface_hover.gamma_multiply(reveal),
            );
        }
        let base = if lit { palette.text } else { palette.secondary };
        let color = if state.usable() {
            base
        } else {
            base.gamma_multiply(0.38)
        }
        .gamma_multiply(reveal);
        let pressed = if response.is_pointer_button_down_on() {
            0.92
        } else {
            1.0
        };
        match state {
            State::Running { fraction, .. } => {
                ring(painter, rect.center(), *fraction, palette, reveal);
                painter.rect_filled(
                    egui::Rect::from_center_size(rect.center(), Vec2::splat(8.0 * pressed)),
                    CornerRadius::same(2),
                    color,
                );
            }
            State::Done(_) => {
                theme::paint_icon(ui, Icon::Check, rect, ICON * pressed, color);
            }
            State::Ready(_) | State::Off(_) => {
                theme::paint_icon(ui, tool.icon(), rect, ICON * pressed, color);
            }
        }
    }
    theme::focus_ring(ui, &response);
    response.on_hover_text(state.tip())
}

/// The four tools, in order. Returns the one used this frame.
pub fn row(ui: &mut Ui, palette: &Palette, states: &[State; 4], reveal: f32) -> Option<Pick> {
    let mut pick = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for (index, (tool, state)) in Tool::ALL.iter().zip(states).enumerate() {
            // Each square follows the one before it by a few hundredths of
            // a second, so the group slides in as a ripple, not a block.
            let delay = index as f32 * 0.15;
            let reveal = ((reveal - delay) / (1.0 - delay)).clamp(0.0, 1.0);
            let response = button(ui, palette, *tool, state, reveal);
            if response.clicked() {
                pick = Some(Pick {
                    tool: *tool,
                    secondary: false,
                });
            } else if response.secondary_clicked() {
                pick = Some(Pick {
                    tool: *tool,
                    secondary: true,
                });
            }
        }
    });
    pick
}

/// A progress ring: the track in the outline colour and the share done in
/// the text colour, from twelve o'clock. Without a known share only the
/// track shows; nothing spins.
fn ring(
    painter: &egui::Painter,
    center: Pos2,
    fraction: Option<f32>,
    palette: &Palette,
    alpha: f32,
) {
    const RADIUS: f32 = 10.0;
    let track = Stroke::new(2.0, palette.outline.gamma_multiply(alpha));
    painter.circle_stroke(center, RADIUS, track);
    let Some(fraction) = fraction.map(|f| f.clamp(0.0, 1.0)) else {
        return;
    };
    if fraction <= 0.0 {
        return;
    }
    let steps = (fraction * 48.0).ceil().max(2.0) as usize;
    let points = (0..=steps)
        .map(|step| {
            let angle = -std::f32::consts::FRAC_PI_2
                + std::f32::consts::TAU * fraction * step as f32 / steps as f32;
            center + vec2(angle.cos(), angle.sin()) * RADIUS
        })
        .collect();
    painter.add(egui::Shape::line(
        points,
        Stroke::new(2.0, palette.text.gamma_multiply(alpha)),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Rect, pos2};

    fn frame(
        ctx: &egui::Context,
        input: egui::RawInput,
        states: &[State; 4],
    ) -> (Option<Pick>, Rect) {
        let mut pick = None;
        let mut rect = Rect::NOTHING;
        let mut output = ctx.run_ui(input, |ui| {
            let palette = Palette::dark();
            let response = ui.scope(|ui| row(ui, &palette, states, 1.0));
            pick = response.inner;
            rect = response.response.rect;
        });
        output.textures_delta.clear();
        (pick, rect)
    }

    fn screen() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 100.0))),
            ..Default::default()
        }
    }

    fn click(at: Pos2, button: egui::PointerButton) -> egui::RawInput {
        let mut input = screen();
        input.events = vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos: at,
                button,
                pressed: false,
                modifiers: Default::default(),
            },
        ];
        input
    }

    fn states() -> [State; 4] {
        [
            State::Off("Download needs the Siphon helper".into()),
            State::Ready("124 BPM · A minor. Click to copy".into()),
            State::Ready("Credits".into()),
            State::Ready("Add to Night drive".into()),
        ]
    }

    #[test]
    fn the_four_tools_keep_their_order_and_width() {
        assert_eq!(
            Tool::ALL.map(Tool::icon),
            [
                Icon::Download,
                Icon::Activity,
                Icon::FileText,
                Icon::ListPlus
            ]
        );
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let (_, rect) = frame(&ctx, screen(), &states());
        assert_eq!(rect.width(), ROW_WIDTH);
        assert_eq!(rect.height(), SIZE);
    }

    /// A click reports its tool, a right-click asks for options, and a tool
    /// that is off reports nothing however it is clicked.
    #[test]
    fn clicks_report_the_tool_and_how() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let (_, rect) = frame(&ctx, screen(), &states());
        let centre = |index: usize| {
            pos2(
                rect.left() + SIZE / 2.0 + index as f32 * (SIZE + GAP),
                rect.center().y,
            )
        };
        let (pick, _) = frame(
            &ctx,
            click(centre(1), egui::PointerButton::Primary),
            &states(),
        );
        assert_eq!(
            pick,
            Some(Pick {
                tool: Tool::BpmKey,
                secondary: false
            })
        );
        let (pick, _) = frame(
            &ctx,
            click(centre(3), egui::PointerButton::Secondary),
            &states(),
        );
        assert_eq!(
            pick,
            Some(Pick {
                tool: Tool::AddToLast,
                secondary: true
            })
        );
        let (pick, _) = frame(
            &ctx,
            click(centre(0), egui::PointerButton::Primary),
            &states(),
        );
        assert_eq!(pick, None, "the helper is off");
    }

    #[test]
    fn a_running_tool_can_still_be_stopped() {
        let running = State::Running {
            fraction: Some(0.64),
            tip: "Downloading WAV. Click to cancel".into(),
        };
        assert!(running.usable());
        assert_eq!(running.tip(), "Downloading WAV. Click to cancel");
        assert!(!State::Off("Not available in your country".into()).usable());
    }
}
