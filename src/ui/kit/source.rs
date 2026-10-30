//! Which service a song comes from, and the colour its title takes.
//!
//! Siphon marks the source on the title itself, never with a chip. The
//! colours are nudged apart so SoundCloud's orange leans yellow and
//! YouTube's red leans crimson: the two never read as one colour.

use egui::{Color32, FontId, Response, Sense, Ui, vec2};

use crate::theme::Palette;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Spotify,
    SoundCloud,
    YouTube,
}

impl Source {
    /// The service behind `uri`, as playback decides it.
    pub fn of(uri: &str) -> Self {
        match crate::soundcloud::Source::of(uri) {
            crate::soundcloud::Source::Spotify => Self::Spotify,
            crate::soundcloud::Source::SoundCloud => Self::SoundCloud,
            crate::soundcloud::Source::YouTube => Self::YouTube,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Spotify => "Spotify",
            Self::SoundCloud => "SoundCloud",
            Self::YouTube => "YouTube",
        }
    }

    /// The title colour on the window and on panels. Light themes take
    /// darker shades so the title keeps its contrast on white.
    pub fn color(self, palette: &Palette) -> Color32 {
        match (self, palette.dark) {
            (Self::Spotify, true) => Color32::from_rgb(0x1e, 0xd7, 0x60),
            (Self::SoundCloud, true) => Color32::from_rgb(0xff, 0x8a, 0x1f),
            (Self::YouTube, true) => Color32::from_rgb(0xff, 0x48, 0x48),
            (Self::Spotify, false) => Color32::from_rgb(0x09, 0x7f, 0x3a),
            (Self::SoundCloud, false) => Color32::from_rgb(0xb8, 0x4d, 0x00),
            (Self::YouTube, false) => Color32::from_rgb(0xcc, 0x1f, 0x26),
        }
    }

    /// The title colour over artwork, which is always darkened behind text
    /// (now playing, the group's newest share), whatever the theme.
    pub fn on_art(self) -> Color32 {
        match self {
            Self::Spotify => Color32::from_rgb(0x3e, 0xe2, 0x7a),
            Self::SoundCloud => Color32::from_rgb(0xff, 0x9a, 0x3d),
            Self::YouTube => Color32::from_rgb(0xff, 0x5a, 0x5a),
        }
    }
}

/// A song title in its source's colour, on one line, cut with an ellipsis
/// when it does not fit. An unavailable song's title drops to the dim text
/// colour, so only songs that can play carry a source colour.
pub fn title(
    ui: &mut Ui,
    palette: &Palette,
    uri: &str,
    text: &str,
    font: FontId,
    available: bool,
) -> Response {
    let color = if available {
        Source::of(uri).color(palette)
    } else {
        palette.dim
    };
    let width = ui.available_width().max(0.0);
    let galley = crate::ui::widgets::ellipsized(ui, text, font, color, width, 1);
    let (rect, response) = ui.allocate_exact_size(vec2(width, galley.size().y), Sense::hover());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), text));
    if ui.is_rect_visible(rect) {
        ui.painter()
            .galley(crate::bidi::galley_pos(rect, &galley), galley, color);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(color: Color32) -> f32 {
        let channel = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// Hue in degrees as the sRGB values read, the way a colour picker
    /// shows it (egui's `Hsva` works in linear light instead).
    fn hue(color: Color32) -> f32 {
        let [r, g, b] = [color.r(), color.g(), color.b()].map(|c| c as f32);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let hue = if max == min {
            0.0
        } else if max == r {
            60.0 * ((g - b) / (max - min))
        } else if max == g {
            60.0 * ((b - r) / (max - min) + 2.0)
        } else {
            60.0 * ((r - g) / (max - min) + 4.0)
        };
        hue.rem_euclid(360.0)
    }

    #[test]
    fn each_uri_finds_its_service() {
        assert_eq!(
            Source::of("spotify:track:4uLU6hMCjMI75M1A2tKUQC"),
            Source::Spotify
        );
        assert_eq!(Source::of("soundcloud:track:123456"), Source::SoundCloud);
        assert_eq!(Source::of("youtube:video:dQw4w9WgXcQ"), Source::YouTube);
        assert_eq!(Source::of(""), Source::Spotify);
    }

    /// Titles are body text, so they need the same 4.5:1 as any text, on
    /// both built-in themes' windows and panels.
    #[test]
    fn titles_are_readable_on_both_themes() {
        for palette in [Palette::dark(), Palette::light()] {
            for source in [Source::Spotify, Source::SoundCloud, Source::YouTube] {
                for ground in [palette.window, palette.panel] {
                    let ratio = contrast(source.color(&palette), ground);
                    assert!(
                        ratio >= 4.5,
                        "{} on {:?} is {ratio:.2}:1",
                        source.name(),
                        ground
                    );
                }
            }
        }
    }

    /// The owner asked for orange and red that never read as one colour.
    #[test]
    fn orange_and_red_stay_apart() {
        for palette in [Palette::dark(), Palette::light()] {
            let orange = hue(Source::SoundCloud.color(&palette));
            let red = hue(Source::YouTube.color(&palette));
            let red = if red > 180.0 { red - 360.0 } else { red };
            assert!(orange - red >= 25.0, "orange {orange:.0}°, red {red:.0}°");
        }
        let orange = hue(Source::SoundCloud.on_art());
        let red = hue(Source::YouTube.on_art());
        assert!(
            orange - red >= 25.0,
            "on art: orange {orange:.0}°, red {red:.0}°"
        );
    }
}
