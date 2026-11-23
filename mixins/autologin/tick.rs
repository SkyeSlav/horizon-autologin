// Each client tick on a server: the delay runs out here, and the login or
// register command goes out.
fn autologin_tick(game: &mut crate::game::Game<'_>) {
    use autologin_engine as al;
    let Some(r) = game.remote.as_ref() else {
        return;
    };
    if !game.native_on(al::ID) || !r.joined || r.reconfiguring.is_some() {
        return;
    }
    // A friend-invite connection has no stable destination: its raw
    // address is "friend:<invite code>", and the shown address is the
    // shared display label "Friend's world". No credential is looked
    // up or sent on it.
    if r.session.address.starts_with("friend:") {
        return;
    }
    let key = (r.session.address.clone(), r.profile_name.clone());
    {
        let game = &*game;
        al::load(&|k| game.native_setting(al::ID, k));
    }
    let out = al::tick(Some((&key.0, &key.1)));
    for (k, v) in al::take_store() {
        game.native_set_setting(al::ID, &k, &v);
    }
    // send_command, not send_chat: a mod's command, sent bare (no slash),
    // signed the way the game signs a typed command.
    if let Some(command) = out.send {
        game.send_command(&command);
    }
    for line in out.lines {
        game.native_chat(&line);
    }
}
