//! Horizon Voice Chat: a client for the Simple Voice Chat protocol (an
//! unofficial one, not affiliated with Simple Voice Chat or its author), speaking it so a Horizon player talks with players on the
//! Java mod on any server that has the voice chat plugin or mod.
//!
//! The game thread calls [`tick`], [`payload`], [`key`], [`command`] and the
//! HUD and settings functions; the voice runs on threads of its own.
pub mod audio;
pub mod buf;
pub mod crypt;
pub mod dsp;
pub mod labels;
pub mod look;
pub mod mix;
pub mod net;
pub mod opus;
pub mod proto;
pub mod ui;

use audio::{AudioIo, Devices, MicQueue};
use mix::{Listener, Mixer};
use net::{Conn, Mode, Redirect, Shared, Status};
use proto::{COMPAT_26, COMPAT_27, Group, PlayerState, Secret};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const ID: &str = "voicechat";
pub const NAME: &str = "Horizon Voice Chat";
pub const VERSION: &str = "1.0.0";
pub const DESCRIPTION: &str = "Talk with players who use the Simple Voice Chat mod, on servers that have it (unofficial). \
V opens the menu: controls, hotkeys and groups. Push to talk (Caps Lock) or voice activation.";

pub const KEY_PTT: &str = "push_to_talk";
pub const KEY_WHISPER: &str = "whisper";
pub const KEY_MUTE: &str = "mute";
pub const KEY_DISABLE: &str = "disable";
pub const DEFAULT_PTT: &str = "key.keyboard.caps.lock";
pub const DEFAULT_WHISPER: &str = "key.keyboard.unknown";
pub const DEFAULT_MUTE: &str = "key.keyboard.j";
pub const DEFAULT_DISABLE: &str = "key.keyboard.k";
pub const KEY_MENU: &str = "menu";
pub const DEFAULT_MENU: &str = "key.keyboard.v";

/// The mod's keys: their name, what they do, and their default.
pub const HOTKEYS: [(&str, &str, &str); 5] = [
    (KEY_MENU, "Open this menu", DEFAULT_MENU),
    (KEY_PTT, "Push to talk", DEFAULT_PTT),
    (KEY_WHISPER, "Whisper (nearby only)", DEFAULT_WHISPER),
    (KEY_MUTE, "Mute / unmute mic", DEFAULT_MUTE),
    (KEY_DISABLE, "Voice chat off / on", DEFAULT_DISABLE),
];

/// A player's eyes above their feet (`Player.getEyeHeight`, standing).
pub const EYE_HEIGHT: f64 = 1.62;

/// The game's side of a tick in a server's world.
pub struct World<'a> {
    /// The address the player joined, as typed (`host` or `host:port`).
    pub address: &'a str,
    pub listener: Option<Listener>,
    /// Other players by UUID, at their eyes.
    pub players: Vec<(u128, [f64; 3])>,
    pub redirect: Redirect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    PushToTalk,
    Whisper,
    Mute,
    Disable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Auto,
    V26,
    V27,
}

/// The settings page and what is stored of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub mode: Mode,
    pub muted: bool,
    pub disabled: bool,
    /// Hear nobody, but stay heard.
    pub deafened: bool,
    /// Mic clean-up: fade background noise, even out the volume.
    pub clean: bool,
    pub volume: u32,
    pub mic_gain: u32,
    pub threshold: i32,
    pub protocol: Protocol,
    pub hud: bool,
    pub group_labels: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // No Caps Lock on a touch screen: Android starts on voice activation.
            mode: if cfg!(target_os = "android") { Mode::Voice } else { Mode::PushToTalk },
            muted: false,
            disabled: false,
            deafened: false,
            clean: true,
            volume: 100,
            mic_gain: 100,
            threshold: -50,
            protocol: Protocol::Auto,
            hud: true,
            group_labels: true,
        }
    }
}

const VOLUMES: [u32; 9] = [0, 25, 50, 75, 100, 125, 150, 175, 200];
const GAINS: [u32; 8] = [50, 75, 100, 125, 150, 200, 250, 300];
const THRESHOLDS: [i32; 7] = [-70, -60, -50, -45, -40, -30, -20];

fn next<T: Copy + PartialEq>(list: &[T], v: T) -> T {
    let i = list.iter().position(|x| *x == v).map_or(0, |i| (i + 1) % list.len());
    list[i]
}

impl Settings {
    pub fn load(get: &dyn Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let num = |k: &str| get(k).and_then(|v| v.parse::<i64>().ok());
        Self {
            mode: match get("mode").as_deref() {
                Some("voice") => Mode::Voice,
                Some("ptt") => Mode::PushToTalk,
                _ => d.mode,
            },
            muted: get("muted").as_deref() == Some("true"),
            disabled: get("disabled").as_deref() == Some("true"),
            deafened: get("deafened").as_deref() == Some("true"),
            clean: get("clean").as_deref() != Some("false"),
            volume: num("volume").map_or(d.volume, |v| v.clamp(0, 200) as u32),
            mic_gain: num("mic_gain").map_or(d.mic_gain, |v| v.clamp(10, 500) as u32),
            threshold: num("threshold").map_or(d.threshold, |v| v.clamp(-127, 0) as i32),
            protocol: match get("protocol").as_deref() {
                Some("2.6") => Protocol::V26,
                Some("2.7") => Protocol::V27,
                _ => Protocol::Auto,
            },
            hud: get("hud").as_deref() != Some("false"),
            group_labels: get("group_labels").as_deref() != Some("false"),
        }
    }

    pub fn stored(&self) -> Vec<(&'static str, String)> {
        vec![
            ("mode", if self.mode == Mode::Voice { "voice" } else { "ptt" }.into()),
            ("muted", self.muted.to_string()),
            ("disabled", self.disabled.to_string()),
            ("deafened", self.deafened.to_string()),
            ("clean", self.clean.to_string()),
            ("volume", self.volume.to_string()),
            ("mic_gain", self.mic_gain.to_string()),
            ("threshold", self.threshold.to_string()),
            ("protocol", match self.protocol {
                Protocol::Auto => "auto",
                Protocol::V26 => "2.6",
                Protocol::V27 => "2.7",
            }
            .into()),
            ("hud", self.hud.to_string()),
            ("group_labels", self.group_labels.to_string()),
        ]
    }

    /// The settings page's buttons, in order.
    pub fn buttons(&self) -> Vec<String> {
        let on_off = |b: bool| if b { "On" } else { "Off" };
        vec![
            format!("Activation: {}", if self.mode == Mode::Voice { "Voice" } else { "Push to talk" }),
            format!("Microphone: {}", if self.muted { "Muted" } else { "On" }),
            format!("Voice chat: {}", on_off(!self.disabled)),
            format!("Voice volume: {}%", self.volume),
            format!("Microphone gain: {}%", self.mic_gain),
            format!("Voice activation level: {} dB", self.threshold),
            format!("Protocol: {}", match self.protocol {
                Protocol::Auto => "Automatic",
                Protocol::V26 => "2.6.x",
                Protocol::V27 => "2.7",
            }),
            format!("Status on screen: {}", on_off(self.hud)),
            format!("Group names above players: {}", on_off(self.group_labels)),
        ]
    }

    pub fn click(&mut self, button: usize) {
        match button {
            0 => self.mode = if self.mode == Mode::Voice { Mode::PushToTalk } else { Mode::Voice },
            1 => self.muted = !self.muted,
            2 => self.disabled = !self.disabled,
            3 => self.volume = next(&VOLUMES, self.volume),
            4 => self.mic_gain = next(&GAINS, self.mic_gain),
            5 => self.threshold = next(&THRESHOLDS, self.threshold),
            6 => {
                self.protocol = match self.protocol {
                    Protocol::Auto => Protocol::V26,
                    Protocol::V26 => Protocol::V27,
                    Protocol::V27 => Protocol::Auto,
                }
            }
            7 => self.hud = !self.hud,
            8 => self.group_labels = !self.group_labels,
            _ => {}
        }
    }
}

/// One server visit.
struct Session {
    address: String,
    joined: Instant,
    /// The versions asked so far, and when the next is due.
    asked: Vec<i32>,
    next_ask: Instant,
    /// The version that answered here before.
    remembered: Option<i32>,
    secret: Option<Secret>,
    conn: Option<Conn>,
    audio: Option<AudioIo>,
    states: HashMap<u128, PlayerState>,
    groups: HashMap<u128, Group>,
    group: Option<u128>,
}

impl Session {
    fn new(address: &str, remembered: Option<i32>) -> Self {
        let now = Instant::now();
        Self {
            address: address.to_owned(),
            joined: now,
            asked: Vec::new(),
            // The server knows the player a moment after the join.
            next_ask: now + Duration::from_secs(1),
            remembered,
            secret: None,
            conn: None,
            audio: None,
            states: HashMap::new(),
            groups: HashMap::new(),
            group: None,
        }
    }

    /// The names of a group's members, sorted.
    fn members(&self, group: u128) -> Vec<String> {
        let mut names: Vec<String> = self
            .states
            .values()
            .filter(|p| p.group == Some(group) && !p.disconnected)
            .map(|p| p.name.clone())
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        names
    }

    fn own_name(&self) -> String {
        let own = self.secret.as_ref().map(|s| s.player);
        own.and_then(|id| self.states.get(&id)).map(|p| p.name.clone()).unwrap_or_default()
    }

    fn compat(&self) -> i32 {
        self.secret.as_ref().map_or(COMPAT_26, |s| s.compat)
    }

    /// The versions to ask, in order: the one that answered here before, then
    /// the releases', then 2.7's, twice over for a server still starting.
    fn plan(&self, protocol: Protocol) -> Vec<i32> {
        match protocol {
            Protocol::V26 => vec![COMPAT_26, COMPAT_26, COMPAT_26],
            Protocol::V27 => vec![COMPAT_27, COMPAT_27, COMPAT_27],
            Protocol::Auto => match self.remembered {
                Some(COMPAT_27) => vec![COMPAT_27, COMPAT_26, COMPAT_27, COMPAT_26],
                _ => vec![COMPAT_26, COMPAT_27, COMPAT_26, COMPAT_27],
            },
        }
    }
}

struct Voice {
    settings: Settings,
    loaded: bool,
    session: Option<Session>,
    /// Custom payloads for the server, sent at the tick.
    outbox: Vec<(String, Vec<u8>)>,
    /// Settings to write back, and chat lines for the player.
    store: Vec<(String, String)>,
    chat: Vec<String>,
    shared: Arc<Shared>,
    mixer: Arc<Mutex<Mixer>>,
    mic: Arc<MicQueue>,
    devices: Arc<Mutex<Devices>>,
    redirect: Option<Redirect>,
    menu: ui::Menu,
    /// Each player's voice, in percent, and whether it is muted: what the
    /// menu set, kept between sessions by UUID.
    prefs: HashMap<u128, (u32, bool)>,
    prefs_seen: HashSet<u128>,
    started: Instant,
}

fn pref_key(kind: &str, id: u128) -> String {
    format!("{kind}.{id:032x}")
}

fn voice() -> MutexGuard<'static, Voice> {
    static VOICE: LazyLock<Mutex<Voice>> = LazyLock::new(|| {
        Mutex::new(Voice {
            settings: Settings::default(),
            loaded: false,
            session: None,
            outbox: Vec::new(),
            store: Vec::new(),
            chat: Vec::new(),
            shared: Arc::default(),
            mixer: Arc::default(),
            mic: Arc::default(),
            devices: Arc::new(Mutex::new(Devices::default())),
            redirect: None,
            menu: ui::Menu::default(),
            prefs: HashMap::new(),
            prefs_seen: HashSet::new(),
            started: Instant::now(),
        })
    });
    VOICE.lock().unwrap_or_else(|e| e.into_inner())
}

static SOUND_CARD: AtomicBool = AtomicBool::new(true);

/// For tools and tests: leave the sound card alone, and feed [`mic_queue`]
/// and read [`mixer`] instead.
pub fn use_sound_card(on: bool) {
    SOUND_CARD.store(on, Ordering::Relaxed);
}

pub fn mic_queue() -> Arc<MicQueue> {
    voice().mic.clone()
}

pub fn mixer() -> Arc<Mutex<Mixer>> {
    voice().mixer.clone()
}

/// The settings key a server's remembered version is kept under.
fn compat_key(address: &str) -> String {
    let clean: String = address
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    format!("compat.{clean}")
}

impl Voice {
    fn apply_controls(&self) {
        let s = &self.settings;
        let mut c = self.shared.controls.lock().unwrap();
        c.mode = s.mode;
        c.muted = s.muted;
        c.disabled = s.disabled;
        c.deafened = s.deafened;
        c.denoise = s.clean;
        c.auto_gain = s.clean;
        c.volume = s.volume as f32 / 100.0;
        c.mic_gain = s.mic_gain as f32 / 100.0;
        c.threshold_db = s.threshold as f32;
    }

    fn save(&mut self) {
        self.apply_controls();
        let stored = self.settings.stored();
        self.store.extend(stored.into_iter().map(|(k, v)| (k.to_owned(), v)));
    }

    /// A player's voice set to `volume` percent, or muted, now.
    fn apply_pref(&mut self, id: u128, volume: u32, muted: bool) {
        self.prefs.insert(id, (volume, muted));
        let level = if muted { 0.0 } else { volume as f32 / 100.0 };
        self.shared.controls.lock().unwrap().player_volumes.insert(id, level);
    }

    /// The same, and kept for next time.
    fn set_pref(&mut self, id: u128, volume: u32, muted: bool) {
        self.apply_pref(id, volume, muted);
        self.store.push((pref_key("pv", id), volume.to_string()));
        self.store.push((pref_key("pm", id), muted.to_string()));
    }

    fn pref(&self, id: u128) -> (u32, bool) {
        self.prefs.get(&id).copied().unwrap_or((100, false))
    }

    fn send(&mut self, (channel, data): (&str, Vec<u8>)) {
        self.outbox.push((channel.to_owned(), data));
    }

    /// Tells the server whether the player muted or turned voice chat off.
    fn send_state(&mut self) {
        let Some(compat) = self.session.as_ref().filter(|s| s.secret.is_some()).map(Session::compat) else {
            return;
        };
        let (muted, disabled) = (self.settings.muted, self.settings.disabled);
        self.send(proto::update_state(compat, muted, disabled));
    }

    fn leave(&mut self) {
        // The connection's threads stop as it drops.
        self.session = None;
        self.shared.transmitting.store(false, Ordering::Relaxed);
        self.shared.talking.lock().unwrap().clear();
        *self.shared.status.lock().unwrap() = Status::Idle;
        *self.shared.mic_error.lock().unwrap() = None;
        self.mixer.lock().unwrap().clear();
    }

    fn on_secret(&mut self, data: &[u8]) {
        let Ok(secret) = Secret::parse(data) else {
            self.chat.push("Voice chat: the server sent a secret this version can't read".into());
            return;
        };
        let Some(redirect) = self.redirect else { return };
        let Some(session) = self.session.as_mut() else { return };
        if session.remembered != Some(secret.compat) {
            session.remembered = Some(secret.compat);
            self.store.push((compat_key(&session.address), secret.compat.to_string()));
        }
        // A new secret replaces the connection.
        session.conn = None;
        let server = match proto::split_host_port(&session.address) {
            Some((host, port)) => (host, port.unwrap_or(25565)),
            None => (session.address.clone(), 25565),
        };
        if let Err(e) = opus::check() {
            *self.shared.mic_error.lock().unwrap() = Some(e.clone());
            self.chat.push(format!("Voice chat can't start: {e}"));
        }
        session.secret = Some(secret.clone());
        session.conn = Some(Conn::start(
            secret,
            server,
            redirect,
            self.shared.clone(),
            self.mixer.clone(),
            self.mic.clone(),
        ));
        if session.audio.is_none() && SOUND_CARD.load(Ordering::Relaxed) {
            session.audio = Some(AudioIo::start(self.mixer.clone(), self.mic.clone(), self.devices.clone()));
        }
        self.send_state();
    }

    fn on_payload(&mut self, channel: &str, data: &[u8]) {
        if channel == "voicechat:secret" {
            self.on_secret(data);
            return;
        }
        let Some(session) = self.session.as_mut() else { return };
        let compat = session.compat();
        match channel {
            "voicechat:states" => {
                if let Ok(states) = PlayerState::parse_all(data, compat) {
                    session.states = states.into_iter().map(|s| (s.uuid, s)).collect();
                }
            }
            "voicechat:state" => {
                if let Ok(s) = PlayerState::parse(data, compat) {
                    session.states.insert(s.uuid, s);
                }
            }
            "voicechat:remove_state" => {
                if let Ok(id) = proto::parse_uuid(data) {
                    session.states.remove(&id);
                }
            }
            "voicechat:add_group" => {
                if let Ok(g) = Group::parse(data) {
                    session.groups.insert(g.id, g);
                }
            }
            "voicechat:remove_group" => {
                if let Ok(id) = proto::parse_uuid(data) {
                    session.groups.remove(&id);
                }
            }
            "voicechat:joined_group" => {
                if let Ok((group, wrong_password)) = proto::parse_joined_group(data) {
                    if wrong_password {
                        self.chat.push("Voice chat: wrong group password".into());
                        self.menu.say("Wrong password", true);
                    } else {
                        session.group = group;
                        let name = group.and_then(|g| session.groups.get(&g)).map(|g| g.name.clone());
                        match &name {
                            Some(n) => self.menu.say(format!("Joined {n}"), false),
                            None if group.is_some() => self.menu.say("Joined the group", false),
                            None => self.menu.say("Left the group", false),
                        }
                        self.chat.push(match name {
                            Some(n) => format!("Voice chat: joined the group {n}"),
                            None if group.is_some() => "Voice chat: joined a group".into(),
                            None => "Voice chat: left the group".into(),
                        });
                    }
                }
            }
            _ => {}
        }
    }

    fn tick(&mut self, world: World, get: &dyn Fn(&str) -> Option<String>) {
        if !self.loaded {
            self.loaded = true;
            self.settings = Settings::load(get);
            self.apply_controls();
        }
        self.redirect = Some(world.redirect);
        if self.session.as_ref().is_some_and(|s| s.address != world.address) {
            self.leave();
        }
        if self.session.is_none() {
            let remembered = get(&compat_key(world.address)).and_then(|v| v.parse().ok());
            self.session = Some(Session::new(world.address, remembered));
            self.send(("minecraft:register", proto::register_payload()));
        }
        if let Some(l) = world.listener {
            *self.shared.listener.lock().unwrap() = l;
        }
        *self.shared.players.lock().unwrap() = world.players.into_iter().collect();
        let unseen: Vec<u128> = self
            .session
            .as_ref()
            .map(|s| s.states.keys().copied().filter(|id| !self.prefs_seen.contains(id)).collect())
            .unwrap_or_default();
        for id in unseen {
            self.prefs_seen.insert(id);
            let volume = get(&pref_key("pv", id)).and_then(|v| v.parse::<u32>().ok()).map(|v| v.min(300));
            let muted = get(&pref_key("pm", id)).as_deref() == Some("true");
            if volume.is_some() || muted {
                self.apply_pref(id, volume.unwrap_or(100), muted);
            }
        }

        let protocol = self.settings.protocol;
        let session = self.session.as_mut().expect("made above");
        let now = Instant::now();
        if session.secret.is_none() && now >= session.next_ask {
            let plan = session.plan(protocol);
            if let Some(&compat) = plan.get(session.asked.len()) {
                session.asked.push(compat);
                session.next_ask = now + Duration::from_secs(3);
                self.send(proto::request_secret(compat));
            }
        }
        // The connection timed out: ask again, as the mod reconnects.
        let session = self.session.as_mut().expect("made above");
        let failed = matches!(*self.shared.status.lock().unwrap(), Status::Failed(_));
        if failed && session.secret.is_some() && now - session.joined > Duration::from_secs(5) {
            let compat = session.compat();
            session.secret = None;
            session.conn = None;
            session.asked.clear();
            session.next_ask = now + Duration::from_secs(5);
            session.remembered = Some(compat);
        }
    }
}

/// A tick in a server's world (`world`), or out of one (None). Returns the
/// custom payloads to send the server and the settings to store.
pub fn tick(world: Option<World>, get: &dyn Fn(&str) -> Option<String>) -> Tick {
    let mut v = voice();
    match world {
        Some(w) => v.tick(w, get),
        None => {
            if v.session.is_some() {
                v.leave();
            }
        }
    }
    Tick {
        payloads: std::mem::take(&mut v.outbox),
        store: std::mem::take(&mut v.store),
        chat: std::mem::take(&mut v.chat),
    }
}

#[derive(Default)]
pub struct Tick {
    pub payloads: Vec<(String, Vec<u8>)>,
    pub store: Vec<(String, String)>,
    pub chat: Vec<String>,
}

/// A custom payload from the server: true when it was the voice chat's.
pub fn payload(channel: &str, data: &[u8]) -> bool {
    if !channel.starts_with("voicechat:") {
        return false;
    }
    voice().on_payload(channel, data);
    true
}

/// A key went down or up. Returns a line for the chat, for toggles.
pub fn key(which: Key, down: bool) -> Option<String> {
    let mut v = voice();
    match which {
        Key::PushToTalk => v.shared.controls.lock().unwrap().ptt = down,
        Key::Whisper => v.shared.controls.lock().unwrap().whisper = down,
        Key::Mute if down => {
            v.settings.muted = !v.settings.muted;
            v.save();
            v.send_state();
            return Some(if v.settings.muted { "Microphone muted" } else { "Microphone on" }.into());
        }
        Key::Disable if down => {
            v.settings.disabled = !v.settings.disabled;
            v.save();
            v.send_state();
            if v.settings.disabled {
                v.mixer.lock().unwrap().clear();
            }
            return Some(if v.settings.disabled { "Voice chat off" } else { "Voice chat on" }.into());
        }
        _ => {}
    }
    None
}

pub fn settings_buttons() -> Vec<String> {
    voice().settings.buttons()
}

pub fn settings_click(button: usize) {
    voice().click(button);
}

impl Voice {
    fn click(&mut self, button: usize) {
        let before = (self.settings.muted, self.settings.disabled);
        self.settings.click(button);
        self.save();
        if before != (self.settings.muted, self.settings.disabled) {
            self.send_state();
        }
    }

    fn status_line(&self) -> (String, u32) {
        let Some(s) = self.session.as_ref() else {
            return ("Not on a server".into(), GREY);
        };
        match &s.secret {
            None if s.asked.len() >= s.plan(self.settings.protocol).len() => ("No voice chat on this server".into(), GREY),
            None => ("Asking the server...".into(), YELLOW),
            Some(secret) => {
                let version = if secret.compat >= COMPAT_27 { "2.7" } else { "2.6.x" };
                match &*self.shared.status.lock().unwrap() {
                    Status::Connected => (format!("Connected (protocol {version})"), GREEN),
                    Status::Failed(e) => (e.clone(), RED),
                    _ => ("Connecting...".into(), YELLOW),
                }
            }
        }
    }

    fn view(&self, hotkeys: Vec<(String, String)>, head_of: &dyn Fn(&str) -> Option<look::HeadPixels>) -> ui::View {
        let session = self.session.as_ref();
        let connected = session.is_some_and(|s| s.secret.is_some())
            && *self.shared.status.lock().unwrap() == Status::Connected;
        let mut groups: Vec<ui::GroupRow> = session
            .map(|s| {
                s.groups
                    .values()
                    .filter(|g| !g.hidden || s.group == Some(g.id))
                    .map(|g| ui::GroupRow {
                        id: g.id,
                        name: g.name.clone(),
                        has_password: g.has_password,
                        members: s.members(g.id),
                        ours: s.group == Some(g.id),
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Yours first, then by name.
        groups.sort_by_key(|g| (!g.ours, g.name.to_lowercase()));
        ui::View {
            status: self.status_line(),
            settings: self.settings.clone(),
            level_db: self.shared.level(),
            connected,
            groups_enabled: session.and_then(|s| s.secret.as_ref()).is_some_and(|s| s.groups),
            groups,
            players: self.player_rows(head_of),
            loopback: self.shared.controls.lock().unwrap().loopback,
            hotkeys,
        }
    }

    /// Who is heard when: talking first, then the player's group, then by name.
    fn player_rows(&self, head_of: &dyn Fn(&str) -> Option<look::HeadPixels>) -> Vec<ui::PlayerRow> {
        let Some(session) = self.session.as_ref() else { return Vec::new() };
        let own = session.secret.as_ref().map(|s| s.player);
        let eye = self.shared.listener.lock().unwrap().eye;
        let at = self.shared.players.lock().unwrap();
        let mut rows: Vec<ui::PlayerRow> = session
            .states
            .values()
            .filter(|p| !p.disconnected && Some(p.uuid) != own)
            .map(|p| {
                let (volume, muted) = self.pref(p.uuid);
                let heard = self.heard(p.uuid);
                ui::PlayerRow {
                    id: p.uuid,
                    name: p.name.clone(),
                    head: look::head_rects(head_of(&p.name).as_ref(), &p.name),
                    talking: heard.is_some(),
                    whisper: heard == Some(true),
                    mic_muted: p.muted,
                    voice_off: p.disabled,
                    in_group: p.group.is_some() && p.group == session.group,
                    distance: at.get(&p.uuid).map(|a| {
                        let d = [a[0] - eye[0], a[1] - eye[1], a[2] - eye[2]];
                        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() as f32
                    }),
                    volume,
                    muted,
                }
            })
            .collect();
        rows.sort_by_key(|r| (!r.talking, !r.in_group, r.name.to_lowercase()));
        rows
    }

    /// Whether `id` was heard in the last moments, and if it was whispering.
    fn heard(&self, id: u128) -> Option<bool> {
        self.shared
            .talking
            .lock()
            .unwrap()
            .get(&id)
            .filter(|(at, _)| at.elapsed() < Duration::from_millis(400))
            .map(|(_, whisper)| *whisper)
    }

    /// What a click or Enter in the menu asks of the voice chat.
    fn menu_action(&mut self, action: ui::Action) {
        use ui::Action as A;
        let Some(action) = self.menu.act(action) else { return };
        let group_name = |v: &Voice, id: u128| {
            v.session.as_ref().and_then(|s| s.groups.get(&id)).map(|g| (g.name.clone(), g.has_password))
        };
        match action {
            A::Mode => self.click(0),
            A::Mute => self.click(1),
            A::Disable => self.click(2),
            A::Labels => self.click(8),
            A::Hud => self.click(7),
            A::Deafen => {
                self.settings.deafened = !self.settings.deafened;
                self.save();
            }
            A::Clean => {
                self.settings.clean = !self.settings.clean;
                self.save();
            }
            A::Loopback => {
                let mut c = self.shared.controls.lock().unwrap();
                c.loopback = !c.loopback;
            }
            A::PlayerStep(id, d) => {
                let (volume, muted) = self.pref(id);
                self.set_pref(id, (volume as i32 + d).clamp(0, 300) as u32, muted);
            }
            A::PlayerMute(id) => {
                let (volume, muted) = self.pref(id);
                self.set_pref(id, volume, !muted);
            }
            A::Slide(slider, f) => {
                let snap = |v: f32, step: f32| (v / step).round() * step;
                match slider {
                    ui::Slider::Volume => self.settings.volume = snap(f * 200.0, 5.0).clamp(0.0, 200.0) as u32,
                    ui::Slider::Gain => self.settings.mic_gain = snap(25.0 + f * 475.0, 5.0).clamp(25.0, 500.0) as u32,
                    ui::Slider::Threshold => self.settings.threshold = (-80.0 + f * 80.0).round().clamp(-80.0, 0.0) as i32,
                }
                self.save();
            }
            A::Volume(d) => {
                self.settings.volume = (self.settings.volume as i32 + d).clamp(0, 200) as u32;
                self.save();
            }
            A::Gain(d) => {
                self.settings.mic_gain = (self.settings.mic_gain as i32 + d).clamp(25, 500) as u32;
                self.save();
            }
            A::Threshold(d) => {
                self.settings.threshold = (self.settings.threshold + d).clamp(-80, 0);
                self.save();
            }
            A::Join(id) => match group_name(self, id) {
                Some((_, true)) => {
                    self.menu.joining = Some(id);
                    self.menu.join_password.clear();
                    self.menu.focus = Some(ui::Field::JoinPassword);
                }
                Some((name, false)) => {
                    self.send(proto::join_group(id, None));
                    self.menu.say(format!("Joining {name}..."), false);
                }
                None => {}
            },
            A::JoinWithPassword => {
                let Some(id) = self.menu.joining.take() else { return };
                let password = std::mem::take(&mut self.menu.join_password);
                self.menu.focus = None;
                if password.is_empty() {
                    self.menu.say("Type the group's password first", true);
                    self.menu.joining = Some(id);
                    self.menu.focus = Some(ui::Field::JoinPassword);
                    return;
                }
                let name = group_name(self, id).map(|(n, _)| n).unwrap_or_default();
                self.send(proto::join_group(id, Some(&password)));
                self.menu.say(format!("Joining {name}..."), false);
            }
            A::Leave => {
                self.send(proto::leave_group());
                self.menu.say("Leaving the group...", false);
            }
            A::Create => {
                let name = self.menu.name.trim().to_owned();
                if name.is_empty() {
                    self.menu.say("Type a name for the group", true);
                    self.menu.focus = Some(ui::Field::Name);
                    return;
                }
                if self.session.as_ref().is_none_or(|s| s.secret.is_none()) {
                    self.menu.say("Voice chat isn't connected", true);
                    return;
                }
                let password = std::mem::take(&mut self.menu.password);
                let password = (!password.is_empty()).then_some(password);
                self.send(proto::create_group(&name, password.as_deref(), self.menu.kind));
                self.menu.name.clear();
                self.menu.focus = None;
                self.menu.say(format!("Creating {name}..."), false);
            }
            _ => {}
        }
    }
}

/// Opens the menu, opened by a key typing `opened_by`.
pub fn menu_open(opened_by: Option<char>) {
    voice().menu.open(opened_by);
}

pub fn menu_is_open() -> bool {
    voice().menu.open
}

/// The game's screen is (`ours`) or isn't the menu.
pub fn menu_sync(ours: bool) {
    voice().menu.sync_open(ours);
}

/// Whether the game should close the menu now (once).
pub fn menu_take_close() -> bool {
    voice().menu.take_close()
}

/// Draws the menu; `hotkeys` are what each key does and its key's name.
pub fn menu_draw(
    size: [f32; 2],
    mouse: [f32; 2],
    measure: &dyn Fn(&str) -> f32,
    hotkeys: Vec<(String, String)>,
    head_of: &dyn Fn(&str) -> Option<look::HeadPixels>,
) -> Vec<ui::Draw> {
    let mut v = voice();
    let view = v.view(hotkeys, head_of);
    v.menu.draw(&view, size, mouse, measure)
}

pub fn menu_click(at: [f32; 2]) {
    let mut v = voice();
    if let Some(action) = v.menu.hit(at) {
        v.menu_action(action);
    }
}

/// A key on the menu: `menu_key` (the open key) closes it unless a field
/// takes the typing.
pub fn menu_key(key: &str, shift: bool, menu_key: &str) {
    let mut v = voice();
    if key == menu_key && !v.menu.typing() {
        v.menu.close();
        return;
    }
    if let Some(action) = v.menu.key(key, shift) {
        v.menu_action(action);
    }
}

pub fn menu_char(c: char) {
    voice().menu.char_typed(c);
}

pub fn menu_scroll(steps: f64) {
    voice().menu.scroll_by(steps.signum() as i32);
}

/// `key.keyboard.v` → `v`: the character a key types, for single letters
/// and digits.
pub fn key_char(key: &str) -> Option<char> {
    let name = key.strip_prefix("key.keyboard.")?;
    let mut chars = name.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

/// Loads the stored settings before the first world, for the settings page.
pub fn load(get: &dyn Fn(&str) -> Option<String>) {
    let mut v = voice();
    if !v.loaded {
        v.loaded = true;
        v.settings = Settings::load(get);
        v.apply_controls();
    }
}

/// Settings changed since the last call, to store.
pub fn take_store() -> Vec<(String, String)> {
    std::mem::take(&mut voice().store)
}

/// What the microphone is doing, for the HUD's icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicState {
    /// All of voice chat is off.
    Off,
    Muted,
    Idle,
    Talking,
    Whispering,
    /// Not connected yet.
    Waiting,
}

/// A player the HUD lists as heard now.
#[derive(Clone, Debug, PartialEq)]
pub struct HudSpeaker {
    pub name: String,
    pub whisper: bool,
    pub grouped: bool,
    pub distance: Option<f32>,
    /// Moves that player's bars apart from the others'.
    pub seed: u32,
}

/// The HUD as the game draws it: icons, a level meter and who is talking.
#[derive(Clone, Debug, PartialEq)]
pub struct HudView {
    pub mic: MicState,
    /// The microphone's level, 0 to 1, and the activation level on the same
    /// scale (voice activation only).
    pub level: f32,
    pub threshold: Option<f32>,
    pub caption: String,
    pub deafened: bool,
    pub group: Option<(String, usize)>,
    /// What is wrong, in red.
    pub problems: Vec<String>,
    pub speakers: Vec<HudSpeaker>,
    pub own_name: String,
    /// Seconds, for the bars' movement.
    pub phase: f32,
}

/// The HUD for the top left corner, or None when there is nothing to show.
pub fn hud_view(ptt_key: &str) -> Option<HudView> {
    let v = voice();
    let session = v.session.as_ref().filter(|s| v.settings.hud && s.secret.is_some())?;
    let status = v.shared.status.lock().unwrap().clone();
    let mut problems = Vec::new();
    match &status {
        Status::Connected => {}
        Status::Failed(e) => problems.push(e.clone()),
        _ => {}
    }
    if let Some(e) = v.shared.mic_error.lock().unwrap().as_ref() {
        problems.push(e.clone());
    }
    if let Err(e) = &v.devices.lock().unwrap().input
        && e != "not open"
    {
        problems.push(format!("Mic: {e}"));
    }
    if let Err(e) = &v.devices.lock().unwrap().output
        && e != "not open"
    {
        problems.push(format!("Speaker: {e}"));
    }
    let level_db = v.shared.level();
    let scale = |db: f32| ((db + 80.0) / 80.0).clamp(0.0, 1.0);
    let transmitting = v.shared.transmitting.load(Ordering::Relaxed);
    let whisper = v.shared.controls.lock().unwrap().whisper;
    let connected = status == Status::Connected;
    let (mic, caption) = if v.settings.disabled {
        (MicState::Off, "Voice chat off".to_owned())
    } else if !connected && problems.is_empty() {
        (MicState::Waiting, "Connecting...".to_owned())
    } else if v.settings.muted {
        (MicState::Muted, "Mic muted".to_owned())
    } else if transmitting && whisper {
        (MicState::Whispering, "Whispering".to_owned())
    } else if transmitting {
        (MicState::Talking, "Talking".to_owned())
    } else {
        (
            MicState::Idle,
            match v.settings.mode {
                Mode::PushToTalk => format!("Hold {}", key_label(ptt_key)),
                Mode::Voice => "Auto".to_owned(),
            },
        )
    };
    let own = session.own_name();
    let group = session
        .group
        .and_then(|g| session.groups.get(&g))
        .map(|g| (g.name.clone(), session.members(g.id).len()));
    let eye = v.shared.listener.lock().unwrap().eye;
    let at = v.shared.players.lock().unwrap();
    let now = Instant::now();
    let mut speakers: Vec<HudSpeaker> = v
        .shared
        .talking
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, (when, _))| now - *when < Duration::from_millis(300))
        .map(|(id, (_, whisper))| {
            let state = session.states.get(id);
            let name = state.map_or_else(|| "Someone".to_owned(), |s| s.name.clone());
            HudSpeaker {
                seed: name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b))),
                grouped: state.is_some_and(|s| s.group.is_some() && s.group == session.group),
                distance: at.get(id).map(|a| {
                    let d = [a[0] - eye[0], a[1] - eye[1], a[2] - eye[2]];
                    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() as f32
                }),
                name,
                whisper: *whisper,
            }
        })
        .collect();
    speakers.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Some(HudView {
        mic,
        level: scale(level_db),
        threshold: (v.settings.mode == Mode::Voice).then(|| scale(v.settings.threshold as f32)),
        caption,
        deafened: v.settings.deafened,
        group,
        problems,
        speakers,
        own_name: own,
        phase: v.started.elapsed().as_secs_f32(),
    })
}

/// The group names and the talking marks to draw above the players the
/// camera sees, on a `size` screen.
pub fn group_labels(camera: &labels::Camera, size: [f32; 2]) -> Vec<labels::Label> {
    let v = voice();
    let Some(session) = v.session.as_ref().filter(|s| v.settings.group_labels && s.secret.is_some()) else {
        return Vec::new();
    };
    let players = v.shared.players.lock().unwrap();
    let shown: Vec<labels::Tag> = players
        .iter()
        .filter_map(|(id, eye)| {
            let group = session.states.get(id).and_then(|p| p.group).and_then(|group| {
                // Hidden groups stay hidden, unless they are the player's own.
                let g = session.groups.get(&group).filter(|g| !g.hidden || session.group == Some(group))?;
                Some((g.name.clone(), session.group == Some(group)))
            });
            let speaking = v.heard(*id);
            (group.is_some() || speaking.is_some()).then(|| labels::Tag { eye: *eye, group, speaking })
        })
        .collect();
    labels::place(camera, size, &shown)
}

const GREY: u32 = 0xAAAAAA;
const GREEN: u32 = 0x55FF55;
const RED: u32 = 0xFF5555;
const YELLOW: u32 = 0xFFFF55;

/// `key.keyboard.caps.lock` → `Caps Lock`.
pub fn key_label(key: &str) -> String {
    let name = key.strip_prefix("key.keyboard.").or_else(|| key.strip_prefix("key.mouse.")).unwrap_or(key);
    if name == "unknown" {
        return "not set".into();
    }
    name.split('.')
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub const COMMAND: &str = "vc";

const HELP: &[&str] = &[
    "/vc status - the connection, devices and who has voice chat",
    "/vc mute, /vc unmute - the microphone",
    "/vc off, /vc on - all of voice chat",
    "/vc deafen, /vc undeafen - hear nobody, but stay heard",
    "/vc mode ptt|voice - push to talk or voice activation",
    "/vc volume <0-200> - everyone's voices",
    "/vc volume <player> <0-200> - one player's voice",
    "/vc gain <10-500> - the microphone's gain",
    "/vc threshold <-127-0> - the voice activation level, in dB",
    "/vc groups - the server's groups",
    "/vc join <group> [password], /vc leave",
    "/vc create <name> [password] [normal|open|isolated]",
];

/// `/vc …`, without the slash: the chat lines it answers, or None when the
/// command isn't the voice chat's.
pub fn command(line: &str) -> Option<Vec<String>> {
    let mut words = line.split_whitespace();
    if words.next() != Some(COMMAND) {
        return None;
    }
    let args: Vec<&str> = words.collect();
    let mut v = voice();
    let out = |s: &str| vec![s.to_owned()];
    Some(match args.as_slice() {
        [] | ["help"] => HELP.iter().map(|s| s.to_string()).collect(),
        ["status"] => v.status(),
        ["mute"] | ["unmute"] => {
            v.settings.muted = args[0] == "mute";
            v.save();
            v.send_state();
            out(if v.settings.muted { "Microphone muted" } else { "Microphone on" })
        }
        ["deafen"] | ["undeafen"] => {
            v.settings.deafened = args[0] == "deafen";
            v.save();
            out(if v.settings.deafened { "You hear nobody now" } else { "You hear voices again" })
        }
        ["off"] | ["on"] => {
            v.settings.disabled = args[0] == "off";
            v.save();
            v.send_state();
            out(if v.settings.disabled { "Voice chat off" } else { "Voice chat on" })
        }
        ["mode", m] => {
            v.settings.mode = match *m {
                "voice" | "va" => Mode::Voice,
                "ptt" | "push" => Mode::PushToTalk,
                _ => return Some(out("Modes: ptt, voice")),
            };
            v.save();
            out(if v.settings.mode == Mode::Voice { "Voice activation" } else { "Push to talk" })
        }
        ["volume", n] => match n.parse::<u32>() {
            Ok(n) if n <= 200 => {
                v.settings.volume = n;
                v.save();
                vec![format!("Voice volume {n}%")]
            }
            _ => out("Volume: 0 to 200"),
        },
        ["volume", name, n] => {
            let Ok(n) = n.parse::<u32>().map(|n| n.min(400)) else {
                return Some(out("Volume: 0 to 400"));
            };
            let id = v.session.as_ref().and_then(|s| {
                s.states.values().find(|p| p.name.eq_ignore_ascii_case(name)).map(|p| p.uuid)
            });
            match id {
                Some(id) => {
                    let muted = v.pref(id).1;
                    v.set_pref(id, n, muted);
                    vec![format!("{name}'s voice at {n}%")]
                }
                None => vec![format!("No voice chat player named {name}")],
            }
        }
        ["gain", n] => match n.parse::<u32>() {
            Ok(n) if (10..=500).contains(&n) => {
                v.settings.mic_gain = n;
                v.save();
                vec![format!("Microphone gain {n}%")]
            }
            _ => out("Gain: 10 to 500"),
        },
        ["threshold", n] => match n.parse::<i32>() {
            Ok(n) if (-127..=0).contains(&n) => {
                v.settings.threshold = n;
                v.save();
                vec![format!("Voice activation at {n} dB (now {:.0} dB)", v.shared.level())]
            }
            _ => out("Threshold: -127 to 0 dB"),
        },
        ["groups"] => {
            let Some(s) = v.session.as_ref().filter(|s| s.secret.is_some()) else {
                return Some(out("Voice chat isn't connected"));
            };
            let mut groups: Vec<&Group> = s.groups.values().filter(|g| !g.hidden || s.group == Some(g.id)).collect();
            groups.sort_by_key(|g| g.name.to_lowercase());
            let lines: Vec<String> = groups
                .into_iter()
                .map(|g| {
                    let members = s.members(g.id);
                    format!(
                        "{}{}{}: {}",
                        g.name,
                        if g.has_password { " (password)" } else { "" },
                        if s.group == Some(g.id) { " (yours)" } else { "" },
                        if members.is_empty() { "nobody".to_owned() } else { members.join(", ") }
                    )
                })
                .collect();
            if lines.is_empty() { out("No groups yet: /vc create <name>") } else { lines }
        }
        ["join", name, rest @ ..] => {
            let found = v.session.as_ref().and_then(|s| {
                s.groups.values().find(|g| g.name.eq_ignore_ascii_case(name)).map(|g| (g.id, g.has_password))
            });
            match found {
                Some((id, has_password)) => {
                    let password = rest.first().copied();
                    if has_password && password.is_none() {
                        return Some(vec![format!("{name} needs a password: /vc join {name} <password>")]);
                    }
                    v.send(proto::join_group(id, password));
                    vec![format!("Joining {name}...")]
                }
                None => vec![format!("No group named {name}: /vc groups")],
            }
        }
        ["leave"] => {
            v.send(proto::leave_group());
            out("Leaving the group...")
        }
        ["create", name, rest @ ..] => {
            let (mut password, mut kind) = (None, 0i16);
            for word in rest {
                match *word {
                    "normal" => kind = 0,
                    "open" => kind = 1,
                    "isolated" => kind = 2,
                    p => password = Some(p),
                }
            }
            if v.session.as_ref().is_none_or(|s| s.secret.is_none()) {
                return Some(out("Voice chat isn't connected"));
            }
            v.send(proto::create_group(name, password, kind));
            vec![format!("Creating the group {name}...")]
        }
        _ => HELP.iter().map(|s| s.to_string()).collect(),
    })
}

impl Voice {
    fn status(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Some(s) = self.session.as_ref() else {
            return vec!["Not on a server".into()];
        };
        match &s.secret {
            None if s.asked.len() >= s.plan(self.settings.protocol).len() => {
                out.push("This server has no Simple Voice Chat (or a version this client doesn't speak)".into())
            }
            None => out.push("Asking the server for voice chat...".into()),
            Some(secret) => {
                let version = if secret.compat >= COMPAT_27 { "2.7" } else { "2.6.x" };
                out.push(format!(
                    "Voice chat {:?} (protocol {version}, distance {:.0} blocks)",
                    *self.shared.status.lock().unwrap(),
                    secret.distance
                ));
            }
        }
        let d = self.devices.lock().unwrap();
        let dev = |r: &Result<String, String>| r.clone().unwrap_or_else(|e| format!("none ({e})"));
        out.push(format!("Microphone: {}, speakers: {}", dev(&d.input), dev(&d.output)));
        if let Some(e) = self.shared.mic_error.lock().unwrap().as_ref() {
            out.push(format!("Problem: {e}"));
        }
        let mut names: Vec<String> = s
            .states
            .values()
            .filter(|p| !p.disconnected)
            .map(|p| format!("{}{}", p.name, if p.disabled { " (off)" } else if p.muted { " (muted)" } else { "" }))
            .collect();
        names.sort();
        if !names.is_empty() {
            out.push(format!("With voice chat: {}", names.join(", ")));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(address: &str) -> World<'_> {
        World { address, listener: None, players: Vec::new(), redirect: |h, p| (h.to_owned(), p) }
    }

    /// The tests share the one voice chat.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn asks_for_the_secret_with_both_versions() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let none = |_: &str| None;
        let t = tick(Some(world("play.example.net")), &none);
        assert_eq!(t.payloads[0].0, "minecraft:register");
        assert!(t.payloads.iter().all(|(c, _)| c != "voicechat:request_secret"));
        voice().session.as_mut().unwrap().next_ask = Instant::now();
        let t = tick(Some(world("play.example.net")), &none);
        assert_eq!(t.payloads, vec![("voicechat:request_secret".into(), 20i32.to_be_bytes().to_vec())]);
        voice().session.as_mut().unwrap().next_ask = Instant::now();
        let t = tick(Some(world("play.example.net")), &none);
        assert_eq!(t.payloads[0].1, 21i32.to_be_bytes().to_vec());
        assert!(!payload("minecraft:brand", b""));
        let t = tick(None, &none);
        assert!(t.payloads.is_empty());
        assert!(voice().session.is_none());
    }

    #[test]
    fn a_players_voice_is_set_and_muted_from_the_menu_and_kept() {
        let mut v = voice();
        v.menu_action(ui::Action::PlayerStep(42, 10));
        v.menu_action(ui::Action::PlayerStep(42, 10));
        assert_eq!(v.pref(42), (120, false));
        assert_eq!(v.shared.controls.lock().unwrap().player_volumes.get(&42), Some(&1.2));
        v.menu_action(ui::Action::PlayerMute(42));
        assert_eq!(v.shared.controls.lock().unwrap().player_volumes.get(&42), Some(&0.0));
        let kept: HashMap<String, String> = v.store.drain(..).collect();
        assert_eq!(kept.get(&pref_key("pv", 42)).map(String::as_str), Some("120"));
        assert_eq!(kept.get(&pref_key("pm", 42)).map(String::as_str), Some("true"));
        // Back again: the volume it had.
        v.menu_action(ui::Action::PlayerMute(42));
        assert_eq!(v.shared.controls.lock().unwrap().player_volumes.get(&42), Some(&1.2));
        v.menu_action(ui::Action::Deafen);
        assert!(v.shared.controls.lock().unwrap().deafened);
        v.menu_action(ui::Action::Deafen);
        assert!(!v.shared.controls.lock().unwrap().deafened);
        v.menu_action(ui::Action::Loopback);
        assert!(v.shared.controls.lock().unwrap().loopback);
        v.menu_action(ui::Action::Loopback);
    }

    #[test]
    fn shows_other_players_groups() {
        use buf::Writer;
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let none = |_: &str| None;
        tick(None, &none);
        let mut w = world("groups.example.net");
        w.players = vec![(0xA, [0.0, 65.62, 10.0]), (0xB, [3.0, 65.62, 12.0])];
        tick(Some(w), &none);
        // As a 2.6 server sends them: a secret, the groups, then the states.
        let mut secret = Writer::new();
        secret.raw(&[1; 16]).i32(24454).uuid(0xC).u8(0).i32(1275);
        secret.raw(&48f64.to_be_bytes()).i32(1000).bool(true).utf("127.0.0.1").bool(false);
        {
            let mut v = voice();
            let s = v.session.as_mut().unwrap();
            s.secret = Some(Secret::parse(&secret.finish()).unwrap());
        }
        for (id, name) in [(0x61u128, "Builders"), (0x62, "Miners")] {
            let mut g = Writer::new();
            g.uuid(id).utf(name).bool(false).bool(true).bool(false).i16(0);
            assert!(payload("voicechat:add_group", &g.finish()));
        }
        let mut states = Writer::new();
        states.i32(3);
        for (id, name, group) in [(0xAu128, "Alex", 0x61u128), (0xB, "Bea", 0x62), (0xC, "Me", 0x61)] {
            states.bool(false).bool(false).uuid(id).utf(name).bool(true).uuid(group);
        }
        assert!(payload("voicechat:states", &states.finish()));
        let mut joined = Writer::new();
        joined.bool(true).uuid(0x61).bool(false);
        payload("voicechat:joined_group", &joined.finish());

        let camera = labels::Camera { pos: [0.0, 65.62, 0.0], yaw: 0.0, pitch: 0.0, fov: 70.0 };
        let shown = group_labels(&camera, [400.0, 200.0]);
        let texts: Vec<_> = shown.iter().map(|l| (l.text.as_str(), l.rgb)).collect();
        assert_eq!(texts, vec![("[Miners]", 0xFFFF55), ("[Builders]", 0x55FF55)]);
        let list = command("vc groups").unwrap();
        assert_eq!(list, vec!["Builders (yours): Alex, Me", "Miners: Bea"]);
        let hud = hud_view("key.keyboard.v").expect("connected");
        assert_eq!(hud.group, Some(("Builders".to_owned(), 2)));

        // The menu: a password group asks for its password, then joins.
        {
            let mut v = voice();
            let s = v.session.as_mut().unwrap();
            s.groups.get_mut(&0x62).unwrap().has_password = true;
        }
        *voice().shared.status.lock().unwrap() = Status::Connected;
        menu_open(Some('v'));
        let measure = |t: &str| t.len() as f32 * 6.0;
        let keys = vec![("Open this menu".to_owned(), "V".to_owned()), ("Push to talk".to_owned(), key_label(DEFAULT_PTT))];
        let has = |d: &[ui::Draw], want: &str| d.iter().any(|d| matches!(d, ui::Draw::Text(t, ..) if t == want));
        let draws = menu_draw([480.0, 270.0], [0.0, 0.0], &measure, keys.clone(), &|_| None);
        assert!(has(&draws, "Voice volume") && has(&draws, "Hold Caps Lock to talk") == (voice().settings.mode == Mode::PushToTalk));
        voice().menu.tab = ui::Tab::Groups;
        let draws = menu_draw([480.0, 270.0], [0.0, 0.0], &measure, keys.clone(), &|_| None);
        assert!(has(&draws, "Miners (password)") && has(&draws, "Alex, Me"));
        let join = voice().menu.hit_rect(&ui::Action::Join(0x62)).unwrap();
        tick(Some(world("groups.example.net")), &none);
        menu_click([join[0] + 1.0, join[1] + 1.0]);
        assert_eq!(voice().menu.joining, Some(0x62));
        menu_char('v');
        for c in "hunter2".chars() {
            menu_char(c);
        }
        menu_key("key.keyboard.enter", false, DEFAULT_MENU);
        let t = tick(Some(world("groups.example.net")), &none);
        assert_eq!(t.payloads, vec![proto::join_group(0x62, Some("hunter2"))].into_iter().map(|(c, d)| (c.to_owned(), d)).collect::<Vec<_>>());
        menu_key(DEFAULT_MENU, false, DEFAULT_MENU);
        assert!(menu_take_close() && !menu_is_open());
        tick(None, &none);
    }

    #[test]
    fn settings_round_trip_and_cycle() {
        let mut s = Settings::default();
        s.click(0);
        s.click(3);
        s.click(6);
        let stored: HashMap<_, _> = s.stored().into_iter().collect();
        let back = Settings::load(&|k| stored.get(k).cloned());
        assert_eq!(back, s);
        assert_eq!((back.mode, back.volume, back.protocol), (Mode::Voice, 125, Protocol::V26));
        assert_eq!(key_label("key.keyboard.caps.lock"), "Caps Lock");
        assert_eq!(compat_key("Play.Example.net:25565"), "compat.play.example.net_25565");
    }
}
