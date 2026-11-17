// `/autologin ...` and `/autoregister`: the mod's own commands, answered
// in the chat. The command arrives without its slash.
fn autologin_command(game: &mut crate::game::Game<'_>, command: &str, ran: &mut bool) {
    use autologin_engine as al;
    if *ran || !game.native_on(al::ID) {
        return;
    }
    let key = game
        .remote
        .as_ref()
        .map(|r| (r.session.address.clone(), r.profile_name.clone()));
    {
        let game = &*game;
        al::load(&|k| game.native_setting(al::ID, k));
    }
    let Some(out) = al::command(
        key.as_ref().map(|(s, p)| (s.as_str(), p.as_str())),
        command,
    ) else {
        return;
    };
    *ran = true;
    if let Some(send) = out.send {
        game.send_command(&send);
    }
    for line in out.lines {
        game.native_chat(&line);
    }
    for (k, v) in al::take_store() {
        game.native_set_setting(al::ID, &k, &v);
    }
}
