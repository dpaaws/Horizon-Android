// Off a server (the title screen, single player, a lost connection): the
// voice chat hangs up.
fn voicechat_frame(game: &mut crate::game::Game<'_>) {
    use voicechat_engine as vc;
    let on_server = game.remote.as_ref().is_some_and(|r| r.joined);
    if !on_server || !game.native_on(vc::ID) {
        vc::tick(None, &|_| None);
    }
}
