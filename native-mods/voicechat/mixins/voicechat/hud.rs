// The voice chat in the top left corner: the microphone as an icon with its
// level, the group, and each player heard now with their face and moving
// bars; and above each player the group and a talking mark, seen from the
// camera the world was drawn from.
fn voicechat_hud(
    game: &mut crate::game::Game<'_>,
    batch: &mut horizon_client::hud::HudBatch,
    corners: &mut horizon_client::extras_hud::Corners,
) {
    use horizon_client::extras::Corner;
    use voicechat_engine as vc;
    use vc::look;
    if !game.native_on(vc::ID) {
        return;
    }
    fn paint(
        game: &crate::game::Game<'_>,
        batch: &mut horizon_client::hud::HudBatch,
        rects: Vec<look::Rect>,
        at: [f32; 2],
        scale: f32,
    ) {
        for (r, argb) in rects {
            game.native_fill(batch, [at[0] + r[0] * scale, at[1] + r[1] * scale, r[2] * scale, r[3] * scale], argb);
        }
    }
    if let Some((pos, yaw, pitch, fov)) = game.locator_camera {
        let camera = vc::labels::Camera { pos, yaw, pitch, fov };
        for label in vc::group_labels(&camera, [batch.width, batch.height]) {
            let icon = if label.speaking.is_some() { 11.0 } else { 0.0 };
            let text_w = if label.text.is_empty() { 0.0 } else { game.native_text_width(&label.text) };
            let w = text_w + icon + 4.0;
            let [x, y] = [label.at[0] - w / 2.0, label.at[1] - 7.0];
            game.native_fill(batch, [x, y, w, 14.0], 0x6000_0000);
            if let Some(whisper) = label.speaking {
                let rgb = if whisper { 0x55FFFF } else { 0x55FF55 };
                paint(game, batch, look::icon_rects(&look::MIC, rgb), [x + 3.0, y + 2.0], 1.0);
            }
            if !label.text.is_empty() {
                game.native_text(batch, &label.text, [x + 2.0 + icon, y + 3.0], label.rgb, true);
            }
        }
    }
    if !game.native_corner_shown(Corner::TopLeft) {
        return;
    }
    let ptt = game.native_mod_key(vc::ID, vc::KEY_PTT, vc::DEFAULT_PTT);
    let Some(view) = vc::hud_view(&ptt) else { return };

    // Measure the rows first: the block is placed once, then drawn.
    const PAD: f32 = 5.0;
    let caption_w = game.native_text_width(&view.caption);
    let mut width = PAD + 14.0 + 40.0 + caption_w + if view.deafened { 14.0 } else { 0.0 } + PAD;
    let mut height = 15.0;
    if let Some((name, count)) = &view.group {
        width = width.max(PAD + 12.0 + game.native_text_width(&format!("{name} ({count})")) + PAD);
        height += 12.0;
    }
    for p in &view.problems {
        width = width.max(PAD + game.native_text_width(p) + PAD);
        height += 11.0;
    }
    for s in &view.speakers {
        let dist = s.distance.map_or(String::new(), |d| format!("{}m", d.round() as i64));
        width = width.max(PAD + 14.0 + game.native_text_width(&s.name) + 6.0 + 9.0 + 6.0 + game.native_text_width(&dist) + PAD);
        height += 12.0;
    }
    let (x, y) = corners.place(Corner::TopLeft, width, height);
    game.native_fill(batch, [x, y, width, height], 0x8000_0000);
    let mic_rgb = match view.mic {
        vc::MicState::Talking => 0x55FF55,
        vc::MicState::Whispering => 0x55FFFF,
        vc::MicState::Idle => 0xFFFFFF,
        vc::MicState::Muted => 0xFF5555,
        vc::MicState::Off => 0xAAAAAA,
        vc::MicState::Waiting => 0xFFFF55,
    };
    game.native_fill(batch, [x, y, 2.0, height], 0xFF00_0000 | mic_rgb);

    // The microphone, its level and what it is doing.
    let mut row = y + 2.0;
    paint(game, batch, look::icon_rects(&look::MIC, mic_rgb), [x + PAD, row], 1.0);
    if matches!(view.mic, vc::MicState::Muted | vc::MicState::Off) {
        paint(game, batch, look::icon_rects(&look::SLASH, 0xFF5555), [x + PAD, row], 1.0);
    }
    let lit = if matches!(view.mic, vc::MicState::Muted | vc::MicState::Off) { 0.0 } else { view.level };
    for i in 0..8 {
        let f = (i as f32 + 0.5) / 8.0;
        let rgb = if f > lit {
            0x3A3A3A
        } else if f < 0.6 {
            0x55FF55
        } else if f < 0.85 {
            0xFFFF55
        } else {
            0xFF5555
        };
        game.native_fill(batch, [x + PAD + 11.0 + i as f32 * 5.0, row + 3.0, 4.0, 5.0], 0xFF00_0000 | rgb);
    }
    if let Some(t) = view.threshold {
        game.native_fill(batch, [x + PAD + 11.0 + t * 40.0, row + 1.0, 1.0, 9.0], 0xFFFF_FFFF);
    }
    let caption_x = x + PAD + 14.0 + 40.0;
    game.native_text(batch, &view.caption, [caption_x, row + 1.0], mic_rgb, true);
    if view.deafened {
        let hx = caption_x + caption_w + 4.0;
        paint(game, batch, look::icon_rects(&look::HEADSET, 0xFFAA00), [hx, row + 1.0], 1.0);
        paint(game, batch, look::icon_rects(&look::SLASH, 0xFF5555), [hx + 1.0, row], 1.0);
    }
    row += 13.0;
    if let Some((name, count)) = &view.group {
        paint(game, batch, look::icon_rects(&look::GROUP, 0x55FF55), [x + PAD, row + 1.0], 1.0);
        game.native_text(batch, &format!("{name} ({count})"), [x + PAD + 12.0, row + 1.0], 0x55FF55, true);
        row += 12.0;
    }
    for p in &view.problems {
        game.native_text(batch, p, [x + PAD, row + 1.0], 0xFF5555, true);
        row += 11.0;
    }
    for s in &view.speakers {
        let rgb = if s.whisper { 0x55FFFF } else { 0xFFFFFF };
        game.native_fill(batch, [x + 2.0, row, width - 2.0, 12.0], if s.whisper { 0x3055FFFF } else { 0x3055FF55 });
        // The face with its hat, one skin pixel a GUI pixel.
        let head = game.native_player_head(&s.name);
        game.native_fill(batch, [x + PAD - 1.0, row + 1.0, 10.0, 10.0], 0xFF00_0000);
        paint(game, batch, look::head_rects(head.as_ref(), &s.name), [x + PAD, row + 2.0], 1.0);
        let name_w = game.native_text_width(&s.name);
        game.native_text(batch, &s.name, [x + PAD + 13.0, row + 2.0], rgb, true);
        // Three bars that move while the voice is heard.
        let bars_x = x + PAD + 13.0 + name_w + 6.0;
        for (i, h) in look::wave_heights(view.phase, s.seed).iter().enumerate() {
            game.native_fill(batch, [bars_x + i as f32 * 3.0, row + 10.0 - h, 2.0, *h], 0xFF00_0000 | if s.whisper { 0x55FFFF } else { 0x55FF55 });
        }
        if let Some(d) = s.distance {
            let text = format!("{}m", d.round() as i64);
            let w = game.native_text_width(&text);
            game.native_text(batch, &text, [x + width - PAD - w, row + 2.0], 0xAAAAAA, true);
        }
        row += 12.0;
    }
}
