// The mod's row in Options > Mods, with its keys.
fn voicechat_listed(mods: &mut Vec<crate::game::native_hooks::NativeMod>) {
    use voicechat_engine as vc;
    mods.push(crate::game::native_hooks::NativeMod {
        id: vc::ID.into(),
        title: format!("{} {}", vc::NAME, vc::VERSION),
        description: vc::DESCRIPTION.into(),
        keys: vc::HOTKEYS.iter().map(|(name, what, default)| [name.to_string(), what.to_string(), default.to_string()]).collect(),
    });
}
