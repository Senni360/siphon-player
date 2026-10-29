//! Editable connection input. Debug output deliberately excludes the token.
use super::account::{InputError, ProfileInput, SessionToken};

pub fn token_field_id() -> egui::Id {
    egui::Id::new("soundcloud-session-token")
}

pub fn clear_token_editor(ctx: &egui::Context) {
    if let Some(mut state) = egui::TextEdit::load_state(ctx, token_field_id()) {
        state.clear_undoer();
        egui::TextEdit::store_state(ctx, token_field_id(), state);
    }
}

pub fn error_message(
    locale: crate::i18n::Locale,
    error: &super::account::ConnectError,
) -> std::borrow::Cow<'static, str> {
    use super::account::{ConnectError, ResolveError};
    use crate::i18n::gettext;
    match error {
        ConnectError::Resolve(ResolveError::NotFound) => {
            gettext(locale, "SoundCloud could not find that profile.")
        }
        ConnectError::Resolve(ResolveError::Unauthorized) => gettext(
            locale,
            "SoundCloud rejected this session. Paste a current session token or connect by profile.",
        ),
        ConnectError::Resolve(ResolveError::Unavailable) => {
            gettext(locale, "SoundCloud could not be reached. Try again.")
        }
        ConnectError::Storage(_) => {
            gettext(locale, "Could not update the protected SoundCloud session.")
        }
        ConnectError::InvalidProfile => gettext(locale, "SoundCloud returned an invalid profile."),
        ConnectError::InvalidSession => gettext(
            locale,
            "The saved SoundCloud session is invalid. Connect again.",
        ),
        ConnectError::DifferentAccount => gettext(
            locale,
            "The saved SoundCloud session belongs to a different account. Connect again.",
        ),
    }
}

#[derive(Clone, Default)]
pub struct Draft {
    pub profile: String,
    pub token: String,
    pub advanced: bool,
    pub busy: bool,
    pub error: Option<String>,
}

impl std::fmt::Debug for Draft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoundCloudDraft")
            .field("advanced", &self.advanced)
            .field("busy", &self.busy)
            .finish_non_exhaustive()
    }
}

pub enum Input {
    Profile(ProfileInput),
    Session(SessionToken),
}

impl Draft {
    pub fn take_input(&mut self) -> Result<Input, InputError> {
        let result = if self.advanced {
            SessionToken::parse(&self.token).map(Input::Session)
        } else {
            ProfileInput::parse(&self.profile).map(Input::Profile)
        };
        // Do not keep a pasted credential in the UI, including after failure.
        self.token.clear();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleared_session_tokens_cannot_be_recovered_with_undo() {
        let ctx = egui::Context::default();
        let cursor = egui::text::CCursorRange::one(egui::text::CCursor::new(0));
        let empty = (cursor, String::new());
        let mut state = egui::text_edit::TextEditState::default();
        let mut undoer = state.undoer();
        undoer.add_undo(&(cursor, "2-000000-dummy_session_token".into()));
        assert!(undoer.has_undo(&empty));
        state.set_undoer(undoer);
        egui::TextEdit::store_state(&ctx, token_field_id(), state);
        clear_token_editor(&ctx);
        let mut undoer = egui::TextEdit::load_state(&ctx, token_field_id())
            .unwrap()
            .undoer();
        assert!(undoer.undo(&empty).is_none());
        assert!(undoer.redo(&empty).is_none());
    }

    #[test]
    fn dialog_defaults_to_public_profile_and_clears_session_drafts() {
        let mut draft = Draft {
            profile: "https://soundcloud.com/listener/likes".into(),
            ..Default::default()
        };
        assert!(matches!(draft.take_input(), Ok(Input::Profile(_))));
        draft.advanced = true;
        draft.token = "OAuth 2-000000-dummy_session_token".into();
        assert!(!format!("{draft:?}").contains("dummy_session_token"));
        assert!(matches!(draft.take_input(), Ok(Input::Session(_))));
        assert!(draft.token.is_empty());
        draft.token = "invalid".into();
        assert!(draft.take_input().is_err());
        assert!(draft.token.is_empty());
    }
}
