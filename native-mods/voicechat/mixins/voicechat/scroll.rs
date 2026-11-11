// The wheel over the voice chat menu scrolls the groups.
fn voicechat_scroll(game: &crate::game::Game<'_>, scroll: [f64; 2], canceled: &mut bool) {
    use voicechat_engine as vc;
    if game.native_mod_screen() != Some(vc::ID) {
        return;
    }
    *canceled = true;
    vc::menu_scroll(scroll[1]);
}
