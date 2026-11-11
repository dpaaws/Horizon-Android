//! Horizon Voice Chat: a client for the Simple Voice Chat protocol (an
//! unofficial one, not affiliated with Simple Voice Chat or its author), speaking it so a Horizon player talks with players on the
//! Java mod on any server that has the voice chat plugin or mod.
//!
//! The game thread calls [`tick`], [`payload`], [`key`], [`command`] and the
//! HUD and settings functions; the voice runs on threads of its own.
pub mod audio;
pub mod buf;
pub mod crypt;
pub mod labels;
pub mod mix;
pub mod net;
pub mod opus;
pub mod proto;
pub mod ui;

use audio::{AudioIo, Devices, MicQueue};
use mix::{Listener, Mixer};
use net::{Conn, Mode, Redirect, Shared, Status};
use proto::{COMPAT_26, COMPAT_27, Group, PlayerState, Secret};
use std::collections::HashMap;
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
    (KEY_WHISPER, "Whisper", DEFAULT_WHISPER),
    (KEY_MUTE, "Mute microphone", DEFAULT_MUTE),
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
        c.volume = s.volume as f32 / 100.0;
        c.mic_gain = s.mic_gain as f32 / 100.0;
        c.threshold_db = s.threshold as f32;
    }

    fn save(&mut self) {
        self.apply_controls();
        let stored = self.settings.stored();
        self.store.extend(stored.into_iter().map(|(k, v)| (k.to_owned(), v)));
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

    fn view(&self, hotkeys: Vec<(String, String)>) -> ui::View {
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
            hotkeys,
        }
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
) -> Vec<ui::Draw> {
    let mut v = voice();
    let view = v.view(hotkeys);
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

/// A line of the status the HUD shows, and its colour.
pub struct Line {
    pub text: String,
    pub rgb: u32,
}

const WHITE: u32 = 0xFFFFFF;
const GREY: u32 = 0xAAAAAA;
const GREEN: u32 = 0x55FF55;
const AQUA: u32 = 0x55FFFF;
const RED: u32 = 0xFF5555;
const YELLOW: u32 = 0xFFFF55;

/// The status for the HUD, top line first: nothing on a server without
/// voice chat.
pub fn hud(ptt_key: &str) -> Vec<Line> {
    let v = voice();
    let Some(session) = v.session.as_ref().filter(|_| v.settings.hud) else {
        return Vec::new();
    };
    if session.secret.is_none() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let line = |text: String, rgb| Line { text, rgb };
    let status = v.shared.status.lock().unwrap().clone();
    match status {
        Status::Connected => {}
        Status::Failed(ref e) => lines.push(line(format!("Voice chat: {e}"), RED)),
        _ => lines.push(line("Voice chat: connecting...".into(), YELLOW)),
    }
    if let Some(e) = v.shared.mic_error.lock().unwrap().as_ref() {
        lines.push(line(format!("Voice chat: {e}"), RED));
    }
    if let Err(e) = &v.devices.lock().unwrap().input
        && e != "not open"
    {
        lines.push(line(format!("Microphone: {e}"), RED));
    }
    if let Err(e) = &v.devices.lock().unwrap().output
        && e != "not open"
    {
        lines.push(line(format!("Speakers: {e}"), RED));
    }
    if v.settings.disabled {
        lines.push(line("Voice chat off".into(), RED));
    } else if v.settings.muted {
        lines.push(line("Microphone muted".into(), RED));
    } else if v.shared.transmitting.load(Ordering::Relaxed) {
        let whisper = v.shared.controls.lock().unwrap().whisper;
        lines.push(if whisper { line("Whispering".into(), AQUA) } else { line("Talking".into(), GREEN) });
    } else if status == Status::Connected {
        lines.push(match v.settings.mode {
            Mode::PushToTalk => line(format!("Push to talk: {}", key_label(ptt_key)), GREY),
            Mode::Voice => line(format!("Voice activation ({:.0} dB)", v.shared.level()), GREY),
        });
    }
    if let Some(g) = session.group.and_then(|g| session.groups.get(&g)) {
        lines.push(line(format!("Group: {}", g.name), GREEN));
        let mut members = session.members(g.id);
        members.retain(|m| *m != session.own_name());
        if !members.is_empty() {
            lines.push(line(format!("  with {}", members.join(", ")), GREY));
        }
    }
    let now = Instant::now();
    let mut talking: Vec<(String, bool)> = v
        .shared
        .talking
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, (at, _))| now - *at < Duration::from_millis(300))
        .map(|(id, (_, whisper))| {
            let name = session.states.get(id).map_or_else(|| "Someone".into(), |s| s.name.clone());
            (name, *whisper)
        })
        .collect();
    talking.sort();
    for (name, whisper) in talking {
        lines.push(line(format!("> {name}"), if whisper { AQUA } else { WHITE }));
    }
    lines
}

/// The group names to draw above the players the camera sees, on a `size`
/// screen.
pub fn group_labels(camera: &labels::Camera, size: [f32; 2]) -> Vec<labels::Label> {
    let v = voice();
    let Some(session) = v.session.as_ref().filter(|s| v.settings.group_labels && s.secret.is_some()) else {
        return Vec::new();
    };
    let players = v.shared.players.lock().unwrap();
    let shown: Vec<([f64; 3], String, bool)> = players
        .iter()
        .filter_map(|(id, eye)| {
            let group = session.states.get(id)?.group?;
            // Hidden groups stay hidden, unless they are the player's own.
            let g = session.groups.get(&group).filter(|g| !g.hidden || session.group == Some(group))?;
            Some((*eye, g.name.clone(), session.group == Some(group)))
        })
        .collect();
    labels::place(camera, size, &shown)
}

/// `key.keyboard.caps.lock` → `Caps Lock`.
pub fn key_label(key: &str) -> String {
    let name = key.strip_prefix("key.keyboard.").or_else(|| key.strip_prefix("key.mouse.")).unwrap_or(key);
    if name == "unknown" {
        return "not bound".into();
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
                    v.shared.controls.lock().unwrap().player_volumes.insert(id, n as f32 / 100.0);
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
        let hud: Vec<String> = hud("key.keyboard.v").into_iter().map(|l| l.text).collect();
        assert!(hud.contains(&"Group: Builders".to_owned()) && hud.contains(&"  with Alex".to_owned()), "{hud:?}");

        // The menu: a password group asks for its password, then joins.
        {
            let mut v = voice();
            let s = v.session.as_mut().unwrap();
            s.groups.get_mut(&0x62).unwrap().has_password = true;
        }
        *voice().shared.status.lock().unwrap() = Status::Connected;
        menu_open(Some('v'));
        let measure = |t: &str| t.len() as f32 * 6.0;
        let keys = vec![("Push to talk".to_owned(), key_label(DEFAULT_PTT))];
        let draws = menu_draw([480.0, 270.0], [0.0, 0.0], &measure, keys.clone());
        let has = |d: &[ui::Draw], want: &str| d.iter().any(|d| matches!(d, ui::Draw::Text(t, ..) if t == want));
        assert!(has(&draws, "Caps Lock") && has(&draws, "Miners (password)") && has(&draws, "Alex, Me"));
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
