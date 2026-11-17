// A click on the settings page: the button's setting moves to its next value.
fn autologin_click(game: &mut crate::game::Game<'_>, mod_id: &str, button: usize) {
    use autologin_engine as al;
    if mod_id != al::ID {
        return;
    }
    {
        let game = &*game;
        al::load(&|k| game.native_setting(al::ID, k));
    }
    al::settings_click(button);
    for (k, v) in al::take_store() {
        game.native_set_setting(al::ID, &k, &v);
    }
}
