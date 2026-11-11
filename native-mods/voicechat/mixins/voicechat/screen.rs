// Draws the voice chat menu: the connection, the controls, the hotkeys as
// bound now, and the groups.
fn voicechat_screen(
    game: &crate::game::Game<'_>,
    batch: &mut horizon_client::hud::HudBatch,
    screen: &crate::game::native_hooks::NativeScreenView<'_>,
) {
    use voicechat_engine as vc;
    if game.native_mod_screen() != Some(vc::ID) {
        return;
    }
    let hotkeys = vc::HOTKEYS
        .iter()
        .map(|(name, what, default)| (what.to_string(), vc::key_label(&game.native_mod_key(vc::ID, name, default))))
        .collect();
    let measure = |t: &str| game.native_text_width(t);
    for draw in vc::menu_draw([batch.width, batch.height], screen.mouse, &measure, hotkeys) {
        match draw {
            vc::ui::Draw::Fill(rect, argb) => game.native_fill(batch, rect, argb),
            vc::ui::Draw::Text(text, at, rgb, shadow) => game.native_text(batch, &text, at, rgb, shadow),
        }
    }
}
