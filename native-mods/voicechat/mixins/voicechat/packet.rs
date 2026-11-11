// A play packet from the server: the voice chat's custom payloads go to the
// voice chat (the game ignores them either way).
fn voicechat_packet(packet: &horizon_protocol::packets::play::Clientbound) {
    if let horizon_protocol::packets::play::Clientbound::CustomPayload(p) = packet {
        voicechat_engine::payload(&p.channel, &p.data);
    }
}
