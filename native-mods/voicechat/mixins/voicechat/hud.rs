// The voice chat's status in the top left corner (connecting, talking,
// muted, the group and who is talking), and each player's group above
// their head, seen from the camera the world was drawn from.
fn voicechat_hud(
    game: &mut crate::game::Game<'_>,
    batch: &mut horizon_client::hud::HudBatch,
    corners: &mut horizon_client::extras_hud::Corners,
) {
    use horizon_client::extras::Corner;
    use voicechat_engine as vc;
    if !game.native_on(vc::ID) {
        return;
    }
    if let Some((pos, yaw, pitch, fov)) = game.locator_camera {
        let camera = vc::labels::Camera { pos, yaw, pitch, fov };
        for label in vc::group_labels(&camera, [batch.width, batch.height]) {
            let w = game.native_text_width(&label.text) + 4.0;
            let [x, y] = [label.at[0] - w / 2.0, label.at[1] - 6.0];
            game.native_fill(batch, [x, y, w, 12.0], 0x6000_0000);
            game.native_text(batch, &label.text, [x + 2.0, y + 2.0], label.rgb, true);
        }
    }
    if !game.native_corner_shown(Corner::TopLeft) {
        return;
    }
    let ptt = game.native_mod_key(vc::ID, vc::KEY_PTT, vc::DEFAULT_PTT);
    for line in vc::hud(&ptt) {
        let w = game.native_text_width(&line.text) + 4.0;
        let (x, y) = corners.place(Corner::TopLeft, w, 12.0);
        game.native_fill(batch, [x, y, w, 12.0], 0x9000_0000);
        game.native_text(batch, &line.text, [x + 2.0, y + 2.0], line.rgb, true);
    }
}
