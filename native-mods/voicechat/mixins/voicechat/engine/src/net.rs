//! The voice connection: a UDP socket to the server's voice port, as
//! `ClientVoicechatConnection` runs it (authenticate, then check the
//! connection, then keep it alive), and the mic thread that sends what the
//! player says.
use crate::audio::MicQueue;
use crate::mix::{Frame, Listener, Mixer, spatial};
use crate::opus::{Decoder, Encoder, FRAME};
use crate::proto::{In, Link, Out, Secret, Sound, SoundKind};
use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Where a server address leads (its SRV record): the game's own resolver.
pub type Redirect = fn(&str, u16) -> (String, u16);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    PushToTalk,
    Voice,
}

/// What the player chose, read by the threads.
#[derive(Clone, Debug)]
pub struct Controls {
    pub mode: Mode,
    pub ptt: bool,
    pub whisper: bool,
    pub muted: bool,
    pub disabled: bool,
    pub threshold_db: f32,
    pub mic_gain: f32,
    pub volume: f32,
    pub player_volumes: HashMap<u128, f32>,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            mode: Mode::PushToTalk,
            ptt: false,
            whisper: false,
            muted: false,
            disabled: false,
            threshold_db: -50.0,
            mic_gain: 1.0,
            volume: 1.0,
            player_volumes: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Connecting,
    Connected,
    Failed(String),
}

/// What the game thread and the voice threads share.
#[derive(Default)]
pub struct Shared {
    pub listener: Mutex<Listener>,
    /// Players' eye positions by UUID, this tick.
    pub players: Mutex<HashMap<u128, [f64; 3]>>,
    /// Who was heard last and when, and whether whispering.
    pub talking: Mutex<HashMap<u128, (Instant, bool)>>,
    pub controls: Mutex<Controls>,
    pub status: Mutex<Status>,
    pub mic_error: Mutex<Option<String>>,
    /// The microphone's last peak, dB as f32 bits.
    pub mic_level: AtomicU32,
    pub transmitting: AtomicBool,
}

impl Shared {
    pub fn level(&self) -> f32 {
        f32::from_bits(self.mic_level.load(Ordering::Relaxed))
    }
    fn set_status(&self, s: Status) {
        *self.status.lock().unwrap() = s;
    }
}

pub struct Conn {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Conn {
    pub fn start(
        secret: Secret,
        server: (String, u16),
        redirect: Redirect,
        shared: Arc<Shared>,
        mixer: Arc<Mutex<Mixer>>,
        mic: Arc<MicQueue>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        shared.set_status(Status::Connecting);
        let thread = std::thread::Builder::new()
            .name("voicechat-net".into())
            .spawn(move || {
                let result = run(secret, server, redirect, &shared, &mixer, mic, &flag);
                mixer.lock().unwrap().clear();
                shared.transmitting.store(false, Ordering::Relaxed);
                shared.set_status(match result {
                    Ok(()) => Status::Idle,
                    Err(e) => Status::Failed(e),
                });
            })
            .ok();
        Self { stop, thread }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn resolve(host: &str, port: u16) -> Result<SocketAddr, String> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("unknown voice host {host}: {e}"))?
        .collect();
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or(addrs.first())
        .copied()
        .ok_or_else(|| format!("unknown voice host {host}"))
}

fn run(
    secret: Secret,
    (server_host, server_port): (String, u16),
    redirect: Redirect,
    shared: &Arc<Shared>,
    mixer: &Arc<Mutex<Mixer>>,
    mic: Arc<MicQueue>,
    stop: &Arc<AtomicBool>,
) -> Result<(), String> {
    // The game connected to the SRV target, if any: so does the voice.
    let (game_host, _) = redirect(&server_host, server_port);
    let (host, port) = secret.voice_address(&game_host);
    let addr = resolve(&host, port)?;
    let socket = UdpSocket::bind(if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })
        .map_err(|e| format!("no UDP socket: {e}"))?;
    socket.connect(addr).map_err(|e| format!("voice server {addr}: {e}"))?;
    socket.set_read_timeout(Some(Duration::from_millis(50))).map_err(|e| e.to_string())?;
    let keep_alive = Duration::from_millis(u64::from(secret.keep_alive) * 10);
    let own = secret.player;
    let link = Arc::new(Link::new(secret));
    let connected = Arc::new(AtomicBool::new(false));

    // The mic thread stops with this one, whatever ends it.
    let quit = Arc::new(AtomicBool::new(false));
    let mic_thread = {
        let (socket, link, shared, connected, quit) =
            (socket.try_clone().map_err(|e| e.to_string())?, link.clone(), shared.clone(), connected.clone(), quit.clone());
        std::thread::Builder::new()
            .name("voicechat-mic".into())
            .spawn(move || mic_loop(&socket, &link, &shared, &mic, &quit, &connected))
            .ok()
    };

    let send = |p: &Out| {
        let _ = socket.send(&link.write(p));
    };
    let mut authenticated = false;
    let mut last_try: Option<Instant> = None;
    let mut last_keep_alive = Instant::now();
    let mut channels: HashMap<u128, Channel> = HashMap::new();
    let mut buf = vec![0u8; 4096];
    let result = loop {
        if stop.load(Ordering::Relaxed) {
            break Ok(());
        }
        let now = Instant::now();
        if !connected.load(Ordering::Relaxed) {
            if last_try.is_none_or(|t| now - t >= Duration::from_secs(1)) {
                last_try = Some(now);
                send(if authenticated { &Out::ConnectionCheck } else { &Out::Authenticate });
            }
        } else if now - last_keep_alive > keep_alive {
            break Err("the voice server stopped answering".into());
        }
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            // Timeouts, and ICMP's "unreachable" until the server is up.
            Err(_) => continue,
        };
        let Ok(Some(packet)) = link.read(&buf[..n]) else { continue };
        match packet {
            In::AuthenticateAck { nonce } => {
                if !authenticated {
                    authenticated = true;
                    *link.nonce.lock().unwrap() = nonce;
                    last_try = None;
                }
            }
            In::ConnectionCheckAck => {
                if authenticated && !connected.swap(true, Ordering::Relaxed) {
                    shared.set_status(Status::Connected);
                    last_keep_alive = now;
                }
            }
            In::KeepAlive => {
                last_keep_alive = now;
                send(&Out::KeepAlive);
            }
            In::Ping { id, timestamp } => send(&Out::Ping { id, timestamp }),
            In::Sound(sound) => {
                if !shared.controls.lock().unwrap().disabled {
                    play(sound, own, shared, mixer, &mut channels);
                }
            }
        }
        channels.retain(|_, c| now - c.used < Duration::from_secs(30));
    };
    connected.store(false, Ordering::Relaxed);
    quit.store(true, Ordering::Relaxed);
    if let Some(t) = mic_thread {
        let _ = t.join();
    }
    result
}

struct Channel {
    decoder: Decoder,
    last: i64,
    used: Instant,
}

/// `AudioChannel.run` and `writeToSpeaker`: decode in order, conceal what
/// was lost, and place the voice where its sender is.
fn play(s: Sound, own: u128, shared: &Shared, mixer: &Mutex<Mixer>, channels: &mut HashMap<u128, Channel>) {
    let now = Instant::now();
    let ch = match channels.entry(s.channel) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(e) => match Decoder::new() {
            Ok(decoder) => e.insert(Channel { decoder, last: -1, used: now }),
            Err(_) => return,
        },
    };
    ch.used = now;
    if s.data.is_empty() {
        // The sender stopped talking.
        ch.last = -1;
        ch.decoder.reset();
        mixer.lock().unwrap().end(s.channel);
        shared.talking.lock().unwrap().remove(&s.sender);
        return;
    }
    let mut frames = Vec::new();
    if s.sequence < 0 {
        // A plugin's raw 16 bit little-endian PCM.
        let pcm: Vec<i16> = s.data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        frames.extend(pcm.chunks(FRAME).map(|c| {
            let mut f = c.to_vec();
            f.resize(FRAME, 0);
            f
        }));
    } else {
        if ch.last >= 0 && s.sequence <= ch.last {
            return;
        }
        let mut lost = if ch.last >= 0 { s.sequence - ch.last - 1 } else { 0 };
        if lost > 5 {
            ch.decoder.reset();
            lost = 0;
        }
        for i in 0..lost {
            frames.push(if i == lost - 1 {
                ch.decoder.decode(Some(&s.data), true)
            } else {
                ch.decoder.decode(None, false)
            });
        }
        frames.push(ch.decoder.decode(Some(&s.data), false));
        ch.last = s.sequence;
    }

    let (volume, listener) = {
        let c = shared.controls.lock().unwrap();
        let v = c.volume * c.player_volumes.get(&s.sender).copied().unwrap_or(1.0);
        (v, *shared.listener.lock().unwrap())
    };
    let gain = match s.kind {
        SoundKind::Group => Some([1.0, 1.0]),
        SoundKind::Entity { .. } if s.sender == own => Some([1.0, 1.0]),
        SoundKind::Entity { distance, .. } => {
            let at = shared.players.lock().unwrap().get(&s.sender).copied();
            at.and_then(|at| spatial(&listener, at, distance))
        }
        SoundKind::Location { at, distance } => spatial(&listener, at, distance),
    };
    let Some(gain) = gain else { return };
    let gain = gain.map(|g| g * volume);
    let mut m = mixer.lock().unwrap();
    for f in frames {
        m.push(s.channel, Frame { pcm: f.iter().map(|&v| f32::from(v) / 32768.0).collect(), gain });
    }
    drop(m);
    if gain.iter().any(|g| *g > 0.0) {
        let whispering = matches!(s.kind, SoundKind::Entity { whispering: true, .. });
        shared.talking.lock().unwrap().insert(s.sender, (now, whispering));
    }
}

/// `MicThread`: each 20 ms frame of the microphone, sent while the push to
/// talk key is held or the voice is loud enough, and a little after.
fn mic_loop(socket: &UdpSocket, link: &Link, shared: &Shared, mic: &MicQueue, stop: &AtomicBool, connected: &AtomicBool) {
    let mut encoder = match Encoder::new(link.secret.opus_application(), link.secret.mtu) {
        Ok(e) => e,
        Err(e) => {
            *shared.mic_error.lock().unwrap() = Some(e);
            return;
        }
    };
    let mut sequence: i64 = 0;
    let mut sending = false;
    let mut hold = 0u32;
    let send = |p: &Out| {
        let _ = socket.send(&link.write(p));
    };
    while !stop.load(Ordering::Relaxed) {
        let frame: Vec<f32> = {
            let mut q = mic.samples.lock().unwrap();
            if q.len() < FRAME {
                q = mic.ready.wait_timeout(q, Duration::from_millis(100)).unwrap().0;
            }
            if q.len() < FRAME {
                continue;
            }
            q.drain(..FRAME).collect()
        };
        let c = shared.controls.lock().unwrap().clone();
        let mut pcm = [0i16; FRAME];
        let mut peak = 0f32;
        for (o, s) in pcm.iter_mut().zip(&frame) {
            let v = (s * c.mic_gain).clamp(-1.0, 1.0);
            peak = peak.max(v.abs());
            *o = (v * 32767.0) as i16;
        }
        let db = if peak > 0.0 { (20.0 * peak.log10()).max(-127.0) } else { -127.0 };
        shared.mic_level.store(db.to_bits(), Ordering::Relaxed);

        let can = connected.load(Ordering::Relaxed) && !c.muted && !c.disabled;
        let mut talk = can
            && match c.mode {
                Mode::PushToTalk => c.ptt || c.whisper,
                Mode::Voice => db > c.threshold_db,
            };
        if talk {
            hold = if c.mode == Mode::PushToTalk { 5 } else { 25 };
        } else if can && hold > 0 {
            hold -= 1;
            talk = true;
        }
        if talk {
            if let Ok(data) = encoder.encode(&pcm) {
                send(&Out::Mic { data, sequence, whispering: c.whisper });
                sequence += 1;
                sending = true;
            }
        } else if sending {
            send(&Out::Mic { data: Vec::new(), sequence, whispering: false });
            sequence += 1;
            encoder.reset();
            sending = false;
            hold = 0;
        }
        shared.transmitting.store(sending, Ordering::Relaxed);
    }
}
