// Off a server (the title screen, single player, a lost connection): the
// voice chat hangs up. And the menu closes when it asked to, and forgets a
// close the game did (Escape, another screen).
fn voicechat_frame(game: &mut crate::game::Game<'_>) {
    use voicechat_engine as vc;
    let on_server = game.remote.as_ref().is_some_and(|r| r.joined);
    if !on_server || !game.native_on(vc::ID) {
        vc::tick(None, &|_| None);
    }
    if vc::menu_take_close() {
        game.native_mod_screen_close(vc::ID);
    }
    vc::menu_sync(game.native_mod_screen() == Some(vc::ID));
}
