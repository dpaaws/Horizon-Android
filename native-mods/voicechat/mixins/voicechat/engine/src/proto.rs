//! Simple Voice Chat's packets, as its client sends and reads them.
//!
//! Two versions are spoken: compatibility version 20 (2.6.x, the releases
//! for 26.3) and 21 (2.7, which adds a random connection id, nonces in the
//! connection check and muted states). The server answers only the version it
//! has, so the client asks with both.
use crate::buf::{Reader, Result, Writer};
use crate::crypt::Crypt;

pub const COMPAT_26: i32 = 20;
pub const COMPAT_27: i32 = 21;

pub const MAGIC: u8 = 0xFF;
pub const MAX_PACKET: usize = 2048;
pub const MAX_OPUS: usize = 1275;
pub const MAX_GROUP_NAME: usize = 24;
pub const MAX_GROUP_PASSWORD: usize = 256;

/// The channels the client registers (`minecraft:register`) so that every
/// server platform sends it the voice chat's packets.
pub const CHANNELS: [&str; 15] = [
    "voicechat:request_secret",
    "voicechat:secret",
    "voicechat:state",
    "voicechat:states",
    "voicechat:remove_state",
    "voicechat:update_state",
    "voicechat:client_state",
    "voicechat:add_group",
    "voicechat:remove_group",
    "voicechat:set_group",
    "voicechat:joined_group",
    "voicechat:leave_group",
    "voicechat:create_group",
    "voicechat:add_category",
    "voicechat:remove_category",
];

pub fn register_payload() -> Vec<u8> {
    CHANNELS.join("\0").into_bytes()
}

pub fn request_secret(compat: i32) -> (&'static str, Vec<u8>) {
    ("voicechat:request_secret", Writer::new().i32(compat).finish())
}

/// `UpdateStatePacket`: 2.6 sends only whether voice chat is off.
pub fn update_state(compat: i32, muted: bool, disabled: bool) -> (&'static str, Vec<u8>) {
    if compat >= COMPAT_27 {
        ("voicechat:client_state", Writer::new().bool(muted).bool(disabled).finish())
    } else {
        ("voicechat:update_state", Writer::new().bool(disabled).finish())
    }
}

pub fn join_group(group: u128, password: Option<&str>) -> (&'static str, Vec<u8>) {
    let mut w = Writer::new();
    w.uuid(group).bool(password.is_some());
    if let Some(p) = password {
        w.utf(&clip(p, MAX_GROUP_PASSWORD));
    }
    ("voicechat:set_group", w.finish())
}

/// Group types: 0 normal, 1 open, 2 isolated.
pub fn create_group(name: &str, password: Option<&str>, kind: i16) -> (&'static str, Vec<u8>) {
    let mut w = Writer::new();
    w.utf(&clip(name, MAX_GROUP_NAME)).bool(password.is_some());
    if let Some(p) = password {
        w.utf(&clip(p, MAX_GROUP_NAME));
    }
    w.i16(kind);
    ("voicechat:create_group", w.finish())
}

pub fn leave_group() -> (&'static str, Vec<u8>) {
    ("voicechat:leave_group", Vec::new())
}

fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// `SecretPacket` and the `InitializationData` it makes.
#[derive(Clone, Debug, PartialEq)]
pub struct Secret {
    pub compat: i32,
    pub key: [u8; 16],
    /// 2.7's connection id; 2.6 names the connection by the player's UUID.
    pub connection_id: Option<u128>,
    pub port: u16,
    pub player: u128,
    pub codec: u8,
    pub mtu: usize,
    pub distance: f64,
    pub keep_alive: u32,
    pub groups: bool,
    pub voice_host: String,
    pub allow_recording: bool,
}

impl Secret {
    /// The secret as either version writes it: the one that reads the whole
    /// payload into sensible values.
    pub fn parse(data: &[u8]) -> Result<Self> {
        Self::parse_as(data, COMPAT_27).or_else(|_| Self::parse_as(data, COMPAT_26))
    }

    pub fn parse_as(data: &[u8], compat: i32) -> Result<Self> {
        let mut r = Reader::new(data);
        let key: [u8; 16] = r.bytes(16)?.try_into().expect("16 bytes");
        let connection_id = if compat >= COMPAT_27 { Some(r.uuid()?) } else { None };
        let port = r.i32()?;
        let player = r.uuid()?;
        let codec = r.u8()?;
        let mtu = r.i32()?;
        let distance = r.f64()?;
        let keep_alive = r.i32()?;
        let groups = r.bool()?;
        let voice_host = r.utf(32767)?;
        let allow_recording = r.u8()?;
        if r.remaining() != 0 {
            return Err("bytes left over");
        }
        if !(0..=65535).contains(&port) || codec > 2 || allow_recording > 1 || keep_alive <= 0 {
            return Err("not a secret of this version");
        }
        if !distance.is_finite() || distance <= 0.0 {
            return Err("not a secret of this version");
        }
        Ok(Self {
            compat,
            key,
            connection_id,
            port: port as u16,
            player,
            codec,
            mtu: mtu.clamp(512, 2048) as usize,
            distance,
            keep_alive: keep_alive as u32,
            groups,
            voice_host,
            allow_recording: allow_recording == 1,
        })
    }

    /// The UUID every UDP packet starts with.
    pub fn header_id(&self) -> u128 {
        self.connection_id.unwrap_or(self.player)
    }

    /// Opus's application for the server's codec setting.
    pub fn opus_application(&self) -> i32 {
        match self.codec {
            1 => crate::opus::APPLICATION_AUDIO,
            2 => crate::opus::APPLICATION_RESTRICTED_LOWDELAY,
            _ => crate::opus::APPLICATION_VOIP,
        }
    }

    /// `InitializationData.parseAddress`: an empty voice host is the game
    /// server's address with the secret's port, a number is only another
    /// port, anything else a host and maybe a port.
    pub fn voice_address(&self, server_host: &str) -> (String, u16) {
        let host = self.voice_host.trim();
        if host.is_empty() {
            return (server_host.to_owned(), self.port);
        }
        if let Ok(port) = host.parse::<i64>() {
            return match u16::try_from(port) {
                Ok(p) if p > 0 => (server_host.to_owned(), p),
                _ => (server_host.to_owned(), self.port),
            };
        }
        match split_host_port(host) {
            Some((h, Some(p))) => (h, p),
            Some((h, None)) => (h, self.port),
            None => (server_host.to_owned(), self.port),
        }
    }
}

/// `host`, `host:port`, `[v6]` or `[v6]:port`, as a URI's authority.
pub fn split_host_port(s: &str) -> Option<(String, Option<u16>)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = after.strip_prefix(':').and_then(|p| p.parse().ok());
        return Some((host.to_owned(), port));
    }
    match s.rsplit_once(':') {
        // A bare IPv6 address has more than one colon and no port.
        Some((h, p)) if !h.contains(':') => Some((h.to_owned(), p.parse().ok())),
        _ => Some((s.to_owned(), None)),
    }
}

/// `PlayerState`: who has voice chat, who turned it off, and their group.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerState {
    pub uuid: u128,
    pub name: String,
    pub muted: bool,
    pub disabled: bool,
    pub disconnected: bool,
    pub group: Option<u128>,
}

impl PlayerState {
    pub fn read(r: &mut Reader, compat: i32) -> Result<Self> {
        if compat >= COMPAT_27 {
            let flags = r.u8()?;
            let uuid = r.uuid()?;
            let name = r.utf(256)?;
            let group = if flags & 0b10_0000 != 0 { Some(r.uuid()?) } else { None };
            Ok(Self {
                uuid,
                name,
                muted: flags & 0b11 != 0,
                disabled: flags & 0b1100 != 0,
                disconnected: flags & 0b1_0000 != 0,
                group,
            })
        } else {
            let disabled = r.bool()?;
            let disconnected = r.bool()?;
            let uuid = r.uuid()?;
            let name = r.utf(32767)?;
            let group = if r.bool()? { Some(r.uuid()?) } else { None };
            Ok(Self { uuid, name, muted: false, disabled, disconnected, group })
        }
    }

    pub fn parse(data: &[u8], compat: i32) -> Result<Self> {
        let mut r = Reader::new(data);
        Self::read(&mut r, compat)
    }

    pub fn parse_all(data: &[u8], compat: i32) -> Result<Vec<Self>> {
        let mut r = Reader::new(data);
        let count = r.i32()?;
        if count < 0 {
            return Err("negative count");
        }
        (0..count).map(|_| Self::read(&mut r, compat)).collect()
    }
}

/// `ClientGroup`.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: u128,
    pub name: String,
    pub has_password: bool,
    pub persistent: bool,
    pub hidden: bool,
    pub kind: i16,
}

impl Group {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        Ok(Self {
            id: r.uuid()?,
            name: r.utf(512)?,
            has_password: r.bool()?,
            persistent: r.bool()?,
            hidden: r.bool()?,
            kind: r.i16()?,
        })
    }
}

/// `JoinedGroupPacket`: the group, or none, and whether the password was wrong.
pub fn parse_joined_group(data: &[u8]) -> Result<(Option<u128>, bool)> {
    let mut r = Reader::new(data);
    let group = if r.bool()? { Some(r.uuid()?) } else { None };
    Ok((group, r.bool()?))
}

pub fn parse_uuid(data: &[u8]) -> Result<u128> {
    Reader::new(data).uuid()
}

/// What the client sends over UDP.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    Mic { data: Vec<u8>, sequence: i64, whispering: bool },
    Authenticate,
    ConnectionCheck,
    KeepAlive,
    Ping { id: u128, timestamp: i64 },
}

/// What the server sends over UDP.
#[derive(Clone, Debug, PartialEq)]
pub enum In {
    Sound(Sound),
    AuthenticateAck { nonce: Option<[u8; 16]> },
    ConnectionCheckAck,
    KeepAlive,
    Ping { id: u128, timestamp: i64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SoundKind {
    /// From a player or entity, heard where it is (`PlayerSoundPacket`).
    Entity { distance: f32, whispering: bool },
    /// The group's, heard as if beside the player (`GroupSoundPacket`).
    Group,
    /// From a place (`LocationSoundPacket`).
    Location { at: [f64; 3], distance: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    pub channel: u128,
    pub sender: u128,
    pub data: Vec<u8>,
    /// Negative: raw 16 bit PCM from a server plugin, not Opus.
    pub sequence: i64,
    pub kind: SoundKind,
    pub category: Option<String>,
}

/// The connection's secret and the version both ends speak.
pub struct Link {
    pub crypt: Crypt,
    pub secret: Secret,
    /// The server's nonce from the authentication ack (2.7).
    pub nonce: std::sync::Mutex<Option<[u8; 16]>>,
}

impl Link {
    pub fn new(secret: Secret) -> Self {
        Self { crypt: Crypt::new(&secret.key), secret, nonce: Default::default() }
    }

    /// `ClientNetworkMessage.writeClient`.
    pub fn write(&self, packet: &Out) -> Vec<u8> {
        let mut body = Writer::new();
        match packet {
            Out::Mic { data, sequence, whispering } => {
                body.u8(0x1).byte_array(data).i64(*sequence).bool(*whispering);
            }
            Out::Authenticate => {
                body.u8(0x5);
                if self.secret.compat < COMPAT_27 {
                    body.uuid(self.secret.player).raw(&self.secret.key);
                }
            }
            Out::Ping { id, timestamp } => {
                body.u8(0x7).uuid(*id).i64(*timestamp);
            }
            Out::KeepAlive => {
                body.u8(0x8);
            }
            Out::ConnectionCheck => {
                body.u8(0x9);
                if self.secret.compat >= COMPAT_27 {
                    body.raw(&self.nonce.lock().unwrap().unwrap_or_default());
                }
            }
        }
        let sealed = self.crypt.encrypt(&body.finish());
        Writer::new().u8(MAGIC).uuid(self.secret.header_id()).byte_array(&sealed).finish()
    }

    /// `ClientNetworkMessage.readPacketClient`. Ok(None) for a packet the
    /// client ignores.
    pub fn read(&self, datagram: &[u8]) -> Result<Option<In>> {
        let mut r = Reader::new(datagram);
        if r.u8()? != MAGIC {
            return Err("not a voice chat packet");
        }
        let sealed = r.byte_array(MAX_PACKET)?;
        let plain = self.crypt.decrypt(sealed).ok_or("not sealed with this secret")?;
        read_plain(&plain)
    }
}

fn read_plain(plain: &[u8]) -> Result<Option<In>> {
    let mut r = Reader::new(plain);
    let packet = match r.u8()? {
        0x2 => {
            let channel = r.uuid()?;
            let sender = r.uuid()?;
            let data = r.byte_array(MAX_OPUS)?.to_vec();
            let sequence = r.i64()?;
            let distance = r.f32()?;
            let flags = r.u8()?;
            let category = if flags & 0b10 != 0 { Some(r.utf(16)?) } else { None };
            In::Sound(Sound {
                channel,
                sender,
                data,
                sequence,
                kind: SoundKind::Entity { distance, whispering: flags & 0b1 != 0 },
                category,
            })
        }
        0x3 => {
            let channel = r.uuid()?;
            let sender = r.uuid()?;
            let data = r.byte_array(MAX_OPUS)?.to_vec();
            let sequence = r.i64()?;
            let flags = r.u8()?;
            let category = if flags & 0b10 != 0 { Some(r.utf(16)?) } else { None };
            In::Sound(Sound { channel, sender, data, sequence, kind: SoundKind::Group, category })
        }
        0x4 => {
            let channel = r.uuid()?;
            let sender = r.uuid()?;
            let at = [r.f64()?, r.f64()?, r.f64()?];
            let data = r.byte_array(MAX_OPUS)?.to_vec();
            let sequence = r.i64()?;
            let distance = r.f32()?;
            let flags = r.u8()?;
            let category = if flags & 0b10 != 0 { Some(r.utf(16)?) } else { None };
            In::Sound(Sound {
                channel,
                sender,
                data,
                sequence,
                kind: SoundKind::Location { at, distance },
                category,
            })
        }
        0x6 => {
            let nonce = if r.remaining() >= 16 {
                Some(r.bytes(16)?.try_into().expect("16 bytes"))
            } else {
                None
            };
            In::AuthenticateAck { nonce }
        }
        0x7 => In::Ping { id: r.uuid()?, timestamp: r.i64()? },
        0x8 => In::KeepAlive,
        0xA => In::ConnectionCheckAck,
        _ => return Ok(None),
    };
    Ok(Some(packet))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret_bytes(compat: i32, voice_host: &str) -> Vec<u8> {
        let mut w = Writer::new();
        w.raw(&[9; 16]);
        if compat >= COMPAT_27 {
            w.uuid(0xC0FFEE);
        }
        w.i32(24454).uuid(0xABCD).u8(0).i32(1275);
        w.raw(&48f64.to_be_bytes()).i32(1000).bool(true).utf(voice_host).bool(false);
        w.finish()
    }

    #[test]
    fn reads_both_secret_versions() {
        let old = Secret::parse(&secret_bytes(COMPAT_26, "")).unwrap();
        assert_eq!((old.compat, old.connection_id, old.port, old.player), (20, None, 24454, 0xABCD));
        assert_eq!(old.header_id(), 0xABCD);
        let new = Secret::parse(&secret_bytes(COMPAT_27, "voice.example.net:5000")).unwrap();
        assert_eq!((new.compat, new.connection_id), (21, Some(0xC0FFEE)));
        assert_eq!(new.header_id(), 0xC0FFEE);
        assert_eq!(new.voice_address("mc.example.net"), ("voice.example.net".into(), 5000));
        assert_eq!(old.voice_address("1.2.3.4"), ("1.2.3.4".into(), 24454));
    }

    #[test]
    fn voice_hosts_parse_as_the_mod_does() {
        let mut s = Secret::parse(&secret_bytes(COMPAT_26, "")).unwrap();
        s.voice_host = "25000".into();
        assert_eq!(s.voice_address("h"), ("h".into(), 25000));
        s.voice_host = "voice.host".into();
        assert_eq!(s.voice_address("h"), ("voice.host".into(), 24454));
        s.voice_host = "[::1]:7".into();
        assert_eq!(s.voice_address("h"), ("::1".into(), 7));
    }

    #[test]
    fn reads_player_states_of_both_versions() {
        let mut w = Writer::new();
        w.i32(1).bool(true).bool(false).uuid(5).utf("Alex").bool(true).uuid(6);
        let old = PlayerState::parse_all(&w.finish(), COMPAT_26).unwrap();
        assert_eq!(old[0], PlayerState {
            uuid: 5,
            name: "Alex".into(),
            muted: false,
            disabled: true,
            disconnected: false,
            group: Some(6),
        });
        let mut w = Writer::new();
        w.u8(0b10_0001).uuid(5).utf("Alex").uuid(6);
        let new = PlayerState::parse(&w.finish(), COMPAT_27).unwrap();
        assert!(new.muted && !new.disabled && new.group == Some(6));
    }

    #[test]
    fn writes_the_authentication_each_version_expects() {
        let link = Link::new(Secret::parse(&secret_bytes(COMPAT_26, "")).unwrap());
        let datagram = link.write(&Out::Authenticate);
        let mut r = Reader::new(&datagram);
        assert_eq!(r.u8().unwrap(), MAGIC);
        assert_eq!(r.uuid().unwrap(), 0xABCD);
        let plain = link.crypt.decrypt(r.byte_array(MAX_PACKET).unwrap()).unwrap();
        assert_eq!(plain.len(), 1 + 16 + 16);
        assert_eq!(&plain[17..], &[9; 16]);

        let link = Link::new(Secret::parse(&secret_bytes(COMPAT_27, "")).unwrap());
        *link.nonce.lock().unwrap() = Some([3; 16]);
        let datagram = link.write(&Out::ConnectionCheck);
        let mut r = Reader::new(&datagram);
        r.u8().unwrap();
        assert_eq!(r.uuid().unwrap(), 0xC0FFEE);
        let plain = link.crypt.decrypt(r.byte_array(MAX_PACKET).unwrap()).unwrap();
        assert_eq!(plain, [&[0x9][..], &[3; 16]].concat());
    }

    #[test]
    fn reads_sound_packets_from_the_server() {
        let link = Link::new(Secret::parse(&secret_bytes(COMPAT_26, "")).unwrap());
        let mut w = Writer::new();
        w.u8(0x2).uuid(1).uuid(2).byte_array(&[1, 2, 3]).i64(7);
        w.raw(&24f32.to_be_bytes()).u8(0b11).utf("music");
        let sealed = link.crypt.encrypt(&w.finish());
        let datagram = Writer::new().u8(MAGIC).byte_array(&sealed).finish();
        let Some(In::Sound(s)) = link.read(&datagram).unwrap() else { panic!() };
        assert_eq!((s.channel, s.sender, s.sequence), (1, 2, 7));
        assert_eq!(s.kind, SoundKind::Entity { distance: 24.0, whispering: true });
        assert_eq!(s.category.as_deref(), Some("music"));
    }

    #[test]
    fn group_payloads_match_the_mod() {
        let (ch, data) = join_group(0x11, Some("pw"));
        assert_eq!(ch, "voicechat:set_group");
        assert_eq!(data.len(), 16 + 1 + 1 + 2);
        let (_, data) = create_group("Friends", None, 1);
        assert_eq!(data, [&b"\x07Friends"[..], &[0], &[0, 1]].concat());
        assert_eq!(update_state(COMPAT_26, true, false), ("voicechat:update_state", vec![0]));
        assert_eq!(update_state(COMPAT_27, true, false), ("voicechat:client_state", vec![1, 0]));
    }
}
