// `/autologin` and `/autoregister` complete and colour as the game's own
// commands do.
fn autologin_commands(commands: &mut Vec<&'static str>) {
    commands.push("autologin");
    commands.push("al");
    commands.push("autoregister");
}
