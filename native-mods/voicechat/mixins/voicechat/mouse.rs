// A click on the voice chat menu: the button or field under the pointer.
fn voicechat_mouse(
    game: &crate::game::Game<'_>,
    screen: &crate::game::native_hooks::NativeScreenView<'_>,
    button: i32,
    canceled: &mut bool,
) {
    use voicechat_engine as vc;
    if game.native_mod_screen() != Some(vc::ID) {
        return;
    }
    *canceled = true;
    if button == 0 {
        vc::menu_click(screen.mouse);
        for (k, v) in vc::take_store() {
            game.native_set_setting(vc::ID, &k, &v);
        }
    }
}
