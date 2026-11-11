// `/vc` completes and colours as the game's own commands do.
fn voicechat_commands(commands: &mut Vec<&'static str>) {
    commands.push(voicechat_engine::COMMAND);
}
