//! Opus from the system's libopus, loaded when voice chat first starts: 48 kHz
//! mono, 20 ms frames, as Simple Voice Chat codes them.
#[cfg(not(target_os = "android"))]
use libloading::Library;
use std::ffi::{c_int, c_void};
use std::sync::OnceLock;

pub const SAMPLE_RATE: u32 = 48_000;
pub const FRAME: usize = 960;

pub const APPLICATION_VOIP: i32 = 2048;
pub const APPLICATION_AUDIO: i32 = 2049;
pub const APPLICATION_RESTRICTED_LOWDELAY: i32 = 2051;
const OPUS_RESET_STATE: c_int = 4028;

type EncoderCreate = unsafe extern "C" fn(i32, c_int, c_int, *mut c_int) -> *mut c_void;
type Encode = unsafe extern "C" fn(*mut c_void, *const i16, c_int, *mut u8, i32) -> i32;
type DecoderCreate = unsafe extern "C" fn(i32, c_int, *mut c_int) -> *mut c_void;
type Decode = unsafe extern "C" fn(*mut c_void, *const u8, i32, *mut i16, c_int, c_int) -> c_int;
type Destroy = unsafe extern "C" fn(*mut c_void);
type Ctl = unsafe extern "C" fn(*mut c_void, c_int, ...) -> c_int;

struct Api {
    #[cfg(not(target_os = "android"))]
    _lib: Library,
    encoder_create: EncoderCreate,
    encode: Encode,
    encoder_destroy: Destroy,
    encoder_ctl: Ctl,
    decoder_create: DecoderCreate,
    decode: Decode,
    decoder_destroy: Destroy,
    decoder_ctl: Ctl,
}

#[cfg(not(target_os = "android"))]
const NAMES: &[&str] = &[
    "libopus.so.0",
    "libopus.so",
    "libopus.0.dylib",
    "libopus.dylib",
    "/opt/homebrew/lib/libopus.0.dylib",
    "/usr/local/lib/libopus.0.dylib",
    "opus.dll",
    "libopus-0.dll",
];

fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(load).as_ref().map_err(Clone::clone)
}

/// Android: the Opus build.rs compiled in.
#[cfg(target_os = "android")]
mod linked {
    use std::ffi::{c_int, c_void};
    unsafe extern "C" {
        pub fn opus_encoder_create(fs: i32, channels: c_int, application: c_int, error: *mut c_int) -> *mut c_void;
        pub fn opus_encode(st: *mut c_void, pcm: *const i16, frame_size: c_int, data: *mut u8, max: i32) -> i32;
        pub fn opus_encoder_destroy(st: *mut c_void);
        pub fn opus_encoder_ctl(st: *mut c_void, request: c_int, ...) -> c_int;
        pub fn opus_decoder_create(fs: i32, channels: c_int, error: *mut c_int) -> *mut c_void;
        pub fn opus_decode(st: *mut c_void, data: *const u8, len: i32, pcm: *mut i16, frame_size: c_int, fec: c_int) -> c_int;
        pub fn opus_decoder_destroy(st: *mut c_void);
        pub fn opus_decoder_ctl(st: *mut c_void, request: c_int, ...) -> c_int;
    }
}

#[cfg(target_os = "android")]
fn load() -> Result<Api, String> {
    use linked::*;
    Ok(Api {
        encoder_create: opus_encoder_create,
        encode: opus_encode,
        encoder_destroy: opus_encoder_destroy,
        encoder_ctl: opus_encoder_ctl,
        decoder_create: opus_decoder_create,
        decode: opus_decode,
        decoder_destroy: opus_decoder_destroy,
        decoder_ctl: opus_decoder_ctl,
    })
}

#[cfg(not(target_os = "android"))]
fn load() -> Result<Api, String> {
    let mut errors = Vec::new();
    for name in NAMES {
        // SAFETY: libopus runs no code of its own when it loads.
        match unsafe { Library::new(name) } {
            Ok(lib) => return unsafe { symbols(lib) }.map_err(|e| format!("{name}: {e}")),
            Err(e) => errors.push(e.to_string()),
        }
    }
    Err(format!("libopus not found (install the opus package): {}", errors.join("; ")))
}

#[cfg(not(target_os = "android"))]
unsafe fn symbols(lib: Library) -> Result<Api, libloading::Error> {
    unsafe {
        Ok(Api {
            encoder_create: *lib.get(b"opus_encoder_create\0")?,
            encode: *lib.get(b"opus_encode\0")?,
            encoder_destroy: *lib.get(b"opus_encoder_destroy\0")?,
            encoder_ctl: *lib.get(b"opus_encoder_ctl\0")?,
            decoder_create: *lib.get(b"opus_decoder_create\0")?,
            decode: *lib.get(b"opus_decode\0")?,
            decoder_destroy: *lib.get(b"opus_decoder_destroy\0")?,
            decoder_ctl: *lib.get(b"opus_decoder_ctl\0")?,
            _lib: lib,
        })
    }
}

/// Whether libopus loads, and why not.
pub fn check() -> Result<(), String> {
    api().map(|_| ())
}

pub struct Encoder {
    api: &'static Api,
    raw: *mut c_void,
    max_bytes: usize,
}

// SAFETY: an encoder is used by one thread at a time.
unsafe impl Send for Encoder {}

impl Encoder {
    /// `max_bytes`: the server's MTU, Opus's 1275 at most.
    pub fn new(application: i32, max_bytes: usize) -> Result<Self, String> {
        let api = api()?;
        let mut err = 0;
        let raw = unsafe { (api.encoder_create)(SAMPLE_RATE as i32, 1, application, &mut err) };
        if raw.is_null() || err != 0 {
            return Err(format!("opus_encoder_create failed ({err})"));
        }
        Ok(Self { api, raw, max_bytes: max_bytes.clamp(64, 1275) })
    }

    pub fn encode(&mut self, frame: &[i16; FRAME]) -> Result<Vec<u8>, String> {
        let mut out = vec![0u8; self.max_bytes];
        let n = unsafe {
            (self.api.encode)(self.raw, frame.as_ptr(), FRAME as c_int, out.as_mut_ptr(), out.len() as i32)
        };
        if n < 0 {
            return Err(format!("opus_encode failed ({n})"));
        }
        out.truncate(n as usize);
        Ok(out)
    }

    pub fn reset(&mut self) {
        unsafe { (self.api.encoder_ctl)(self.raw, OPUS_RESET_STATE) };
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { (self.api.encoder_destroy)(self.raw) };
    }
}

pub struct Decoder {
    api: &'static Api,
    raw: *mut c_void,
}

// SAFETY: a decoder is used by one thread at a time.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new() -> Result<Self, String> {
        let api = api()?;
        let mut err = 0;
        let raw = unsafe { (api.decoder_create)(SAMPLE_RATE as i32, 1, &mut err) };
        if raw.is_null() || err != 0 {
            return Err(format!("opus_decoder_create failed ({err})"));
        }
        Ok(Self { api, raw })
    }

    /// A frame from `packet`; None conceals a lost one, and `fec` rebuilds
    /// the lost frame before `packet` from the redundancy in it.
    pub fn decode(&mut self, packet: Option<&[u8]>, fec: bool) -> Vec<i16> {
        let mut pcm = vec![0i16; FRAME];
        let (ptr, len) = packet.map_or((std::ptr::null(), 0), |p| (p.as_ptr(), p.len() as i32));
        let n = unsafe {
            (self.api.decode)(self.raw, ptr, len, pcm.as_mut_ptr(), FRAME as c_int, fec as c_int)
        };
        if n < 0 {
            pcm.fill(0);
        } else {
            pcm.truncate(n as usize);
            pcm.resize(FRAME, 0);
        }
        pcm
    }

    pub fn reset(&mut self) {
        unsafe { (self.api.decoder_ctl)(self.raw, OPUS_RESET_STATE) };
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { (self.api.decoder_destroy)(self.raw) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_survives_encoding() {
        if check().is_err() {
            eprintln!("no libopus here: skipped");
            return;
        }
        let mut enc = Encoder::new(APPLICATION_VOIP, 1275).unwrap();
        let mut dec = Decoder::new().unwrap();
        let mut last = Vec::new();
        for f in 0..10 {
            let frame: [i16; FRAME] = std::array::from_fn(|i| {
                let t = (f * FRAME + i) as f32 / SAMPLE_RATE as f32;
                ((t * 440.0 * std::f32::consts::TAU).sin() * 8000.0) as i16
            });
            let packet = enc.encode(&frame).unwrap();
            assert!(!packet.is_empty() && packet.len() <= 1275);
            last = dec.decode(Some(&packet), false);
        }
        let peak = last.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak > 4000, "peak {peak}");
        assert_eq!(dec.decode(None, false).len(), FRAME);
    }
}
