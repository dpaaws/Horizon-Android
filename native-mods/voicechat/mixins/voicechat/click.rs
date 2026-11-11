// A click on the settings page: the button's setting moves to its next value.
fn voicechat_click(game: &mut crate::game::Game<'_>, mod_id: &str, button: usize) {
    use voicechat_engine as vc;
    if mod_id != vc::ID {
        return;
    }
    {
        let game = &*game;
        vc::load(&|k| game.native_setting(vc::ID, k));
    }
    vc::settings_click(button);
    for (k, v) in vc::take_store() {
        game.native_set_setting(vc::ID, &k, &v);
    }
}
