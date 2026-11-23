// `/autologin ...` and `/autoregister`: the mod's own commands, answered
// in the chat. The command arrives without its slash.
fn autologin_command(game: &mut crate::game::Game<'_>, command: &str, ran: &mut bool) {
    use autologin_engine as al;
    if *ran || !game.native_on(al::ID) {
        return;
    }
    // A friend-invite connection has no stable destination (see tick.rs):
    // no key, so nothing is looked up or sent — and the engine's "Not on
    // a server." would mislead, so say why instead.
    let friend = game
        .remote
        .as_ref()
        .map_or(false, |r| r.session.address.starts_with("friend:"));
    let key = if friend {
        None
    } else {
        game.remote
            .as_ref()
            .map(|r| (r.session.address.clone(), r.profile_name.clone()))
    };
    {
        let game = &*game;
        al::load(&|k| game.native_setting(al::ID, k));
    }
    let Some(mut out) = al::command(key.as_ref().map(|(s, p)| (s.as_str(), p.as_str())), command)
    else {
        return;
    };
    if friend {
        out.send = None;
        out.lines = vec![
            "[AutoLogin] Friends worlds don't have a stable address, so auto-login is off here."
                .to_string(),
        ];
    }
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
