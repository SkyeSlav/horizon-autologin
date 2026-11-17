// The mod's settings page: auto-detect toggles, the delay, debug messages.
// Patterns are free text, so they are set with
// `/autologin pattern login <text>` instead.
fn autologin_page(game: &mut crate::game::Game<'_>, mod_id: &str, buttons: &mut Vec<String>) {
    use autologin_engine as al;
    if mod_id != al::ID {
        return;
    }
    {
        let game = &*game;
        al::load(&|k| game.native_setting(al::ID, k));
    }
    buttons.extend(al::settings_buttons());
}
