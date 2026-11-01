//! Public identity of the player, separate from the Siphon browser helper.

/// What the app is called wherever a person reads it.
pub const NAME: &str = "Encore";
/// The whole name, for the places that say exactly what this is.
pub const FULL_NAME: &str = "Siphon: Encore";
/// The name the system already knows it by: its login entry. Never changes, or
/// people who start it with their computer would lose that on updating.
pub const STABLE_NAME: &str = "Siphon";
/// Like `DATA_NAME` and `APP_ID`, these name its folders, credentials and
/// single-instance lock, and changing them would strand what people have.
pub const SLUG: &str = "siphon";
pub const APP_ID: &str = "com.siphon.player";
pub const DATA_NAME: &str = "Siphon Player";
