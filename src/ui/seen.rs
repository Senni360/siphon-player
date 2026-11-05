//! What's new from the group: a share or file not looked at yet, or one with a new
//! reply since it was. Kept with the window's other remembered state.

use egui::{Color32, Context, Id, Pos2};

/// How long something must be on screen in the group to count as looked at.
const LOOKED_AT_SECONDS: f64 = 2.5;
/// How many are remembered, the newest kept.
const REMEMBERED: usize = 2000;

fn key() -> Id {
    Id::new("group-looked-at")
}

fn all(ctx: &Context) -> Vec<(String, u32)> {
    ctx.data_mut(|data| data.get_persisted::<Vec<(String, u32)>>(key()))
        .unwrap_or_default()
}

/// Whether this is new: never looked at, or with replies since it was.
pub fn is_new(ctx: &Context, id: &str, replies: u32) -> bool {
    !all(ctx)
        .iter()
        .any(|(seen, read)| seen == id && *read >= replies)
}

/// Marks it looked at, with the replies it has now.
pub fn mark(ctx: &Context, id: &str, replies: u32) {
    let mut seen = all(ctx);
    seen.retain(|(each, _)| each != id);
    seen.push((id.to_owned(), replies));
    if seen.len() > REMEMBERED {
        seen.drain(..seen.len() - REMEMBERED);
    }
    ctx.data_mut(|data| data.insert_persisted(key(), seen));
}

/// Whether the group has been looked at before. On the first visit everything
/// already there counts as seen, or a long history would all read as new.
pub fn first_visit(ctx: &Context) -> bool {
    ctx.data_mut(|data| data.get_persisted::<Vec<(String, u32)>>(key()))
        .is_none()
}

/// Call for each thing drawn on screen in the group: after a moment there, it
/// counts as looked at. Asks for the frame that makes that true.
pub fn looking_at(ctx: &Context, id: &str, replies: u32) {
    if !is_new(ctx, id, replies) {
        return;
    }
    let timer = key().with(("since", id));
    let now = ctx.input(|input| input.time);
    let since = ctx.data_mut(|data| *data.get_temp_mut_or_insert_with(timer, || now));
    if now - since >= LOOKED_AT_SECONDS {
        mark(ctx, id, replies);
        ctx.data_mut(|data| data.remove::<f64>(timer));
    } else {
        ctx.request_repaint_after(std::time::Duration::from_millis(300));
    }
}

/// A small exclamation mark in a disc: this is new.
pub fn badge(painter: &egui::Painter, center: Pos2, color: Color32, on: Color32) {
    painter.circle_filled(center, 9.0, color);
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        "!",
        crate::theme::bold(13.0),
        on,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_reply_makes_something_new_again() {
        let ctx = Context::default();
        assert!(first_visit(&ctx));
        assert!(is_new(&ctx, "post", 0));
        mark(&ctx, "post", 1);
        assert!(!first_visit(&ctx));
        assert!(!is_new(&ctx, "post", 1));
        assert!(is_new(&ctx, "post", 2), "a new reply");
        mark(&ctx, "post", 2);
        assert!(!is_new(&ctx, "post", 2));
        assert!(is_new(&ctx, "other", 0));
    }

    #[test]
    fn being_on_screen_only_counts_after_a_moment() {
        let ctx = Context::default();
        let frame = |time: f64| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ui| looking_at(ui.ctx(), "post", 0),
            );
            output.textures_delta.clear();
        };
        frame(1.0);
        frame(2.0);
        assert!(is_new(&ctx, "post", 0));
        frame(4.0);
        assert!(!is_new(&ctx, "post", 0));
    }
}
