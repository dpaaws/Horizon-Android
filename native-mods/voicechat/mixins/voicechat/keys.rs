// Keys over the voice chat menu are the menu's: typing, Tab, Enter, Escape,
// and the menu key closes it again.
fn voicechat_keys(game: &crate::game::Game<'_>, key: &str, modifiers: u32, canceled: &mut bool) {
    use voicechat_engine as vc;
    if game.native_mod_screen() != Some(vc::ID) {
        return;
    }
    *canceled = true;
    let menu = game.native_mod_key(vc::ID, vc::KEY_MENU, vc::DEFAULT_MENU);
    vc::menu_key(key, modifiers & 1 != 0, &menu);
    for (k, v) in vc::take_store() {
        game.native_set_setting(vc::ID, &k, &v);
    }
}
