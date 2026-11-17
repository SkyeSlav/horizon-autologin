// Every play packet, first: the login/register prompts the server sends.
// Only their plain text is kept, for the engine to match against the
// login/register patterns. SystemChat is where AuthMe-style prompts arrive;
// PlayerChat and DisguisedChat are covered so a prompt sent as either is
// not missed. (One function per helper file: the build server requires it.)
fn autologin_packet(
    packet: &horizon_protocol::packets::play::Clientbound,
    game: &mut crate::game::Game<'_>,
) {
    use autologin_engine as al;
    use horizon_protocol::packets::play::Clientbound as C;
    let text = match packet {
        C::SystemChat(p) if !p.overlay => {
            crate::game::remote::remote_text(&p.content).plain()
        }
        C::PlayerChat(p) => p.content.clone(),
        C::DisguisedChat(p) => crate::game::remote::remote_text(&p.message).plain(),
        _ => return,
    };
    let Some(r) = game.remote.as_ref() else {
        return;
    };
    if !r.joined {
        return;
    }
    al::note_chat(&r.session.address, &r.profile_name, &text);
}
