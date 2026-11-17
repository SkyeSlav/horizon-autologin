// The mod's row in Options > Mods.
fn autologin_listed(mods: &mut Vec<crate::game::native_hooks::NativeMod>) {
    use autologin_engine as al;
    mods.push(crate::game::native_hooks::NativeMod {
        id: al::ID.into(),
        title: format!("{} {}", al::NAME, al::VERSION),
        description: al::DESCRIPTION.into(),
        keys: Vec::new(),
    });
}
