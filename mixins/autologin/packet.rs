// Every play packet, first: the login/register prompts the server sends.
// Only their plain text is kept, for the engine to match against the
// login/register patterns. SystemChat is where AuthMe-style prompts arrive
// (overlay lines included: the action bar is a SystemChat with overlay set);
// PlayerChat and DisguisedChat are covered so a prompt sent as either is
// not missed. Title, subtitle and action-bar prompts arrive as their own
// packets, so those are read too — a server that prompts on the title or
// the action bar instead of chat is otherwise invisible to the mod.
// (One function per helper file: the build server requires it.)
fn autologin_packet(
    packet: &horizon_protocol::packets::play::Clientbound,
    game: &mut crate::game::Game<'_>,
) {
    use autologin_engine as al;
    use horizon_protocol::packets::play::Clientbound as C;
    let text = match packet {
        C::SystemChat(p) => crate::game::remote::remote_text(&p.content).plain(),
        C::PlayerChat(p) => p.content.clone(),
        C::DisguisedChat(p) => crate::game::remote::remote_text(&p.message).plain(),
        C::SetTitleText(p) | C::SetSubtitleText(p) | C::SetActionBarText(p) => {
            crate::game::remote::remote_text(&p.text).plain()
        }
        _ => return,
    };
    let Some(r) = game.remote.as_ref() else {
        return;
    };
    if !r.joined {
        return;
    }
    // A friend-invite connection has no stable destination (see tick.rs):
    // its prompts are not noted, since nothing may be sent on it.
    if r.session.address.starts_with("friend:") {
        return;
    }
    al::note_chat(&r.session.address, &r.profile_name, &text);
}
