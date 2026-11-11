//! Android's sound card: AAudio, from `libaaudio.so` (part of Android since 8.0, loaded here so
//! the library carries no link to it). Both streams run at 48 kHz in float, shared mode; the system
//! converts when the device cannot. The microphone needs the RECORD_AUDIO permission, which
//! Horizon asks the player for (AAUDIO_DIRECTION_INPUT below is what tells its build to ask).
use super::*;
use libloading::Library;
use std::ffi::c_void;

const AAUDIO_DIRECTION_OUTPUT: i32 = 0;
const AAUDIO_DIRECTION_INPUT: i32 = 1;
const AAUDIO_SHARING_MODE_SHARED: i32 = 1;
const AAUDIO_FORMAT_PCM_FLOAT: i32 = 2;
const AAUDIO_PERFORMANCE_MODE_LOW_LATENCY: i32 = 12;
const AAUDIO_USAGE_GAME: i32 = 14;
const AAUDIO_INPUT_PRESET_VOICE_COMMUNICATION: i32 = 7;
const AAUDIO_CALLBACK_RESULT_CONTINUE: i32 = 0;

type Builder = *mut c_void;
type RawStream = *mut c_void;
type DataCallback = unsafe extern "C" fn(RawStream, *mut c_void, *mut c_void, i32) -> i32;
type ErrorCallback = unsafe extern "C" fn(RawStream, *mut c_void, i32);

/// The AAudio functions used.
struct Api {
    _lib: Library,
    create_builder: unsafe extern "C" fn(*mut Builder) -> i32,
    builder_delete: unsafe extern "C" fn(Builder) -> i32,
    set_direction: unsafe extern "C" fn(Builder, i32),
    set_sharing_mode: unsafe extern "C" fn(Builder, i32),
    set_sample_rate: unsafe extern "C" fn(Builder, i32),
    set_channel_count: unsafe extern "C" fn(Builder, i32),
    set_format: unsafe extern "C" fn(Builder, i32),
    set_performance_mode: unsafe extern "C" fn(Builder, i32),
    set_data_callback: unsafe extern "C" fn(Builder, DataCallback, *mut c_void),
    set_error_callback: unsafe extern "C" fn(Builder, ErrorCallback, *mut c_void),
    open_stream: unsafe extern "C" fn(Builder, *mut RawStream) -> i32,
    stream_close: unsafe extern "C" fn(RawStream) -> i32,
    request_start: unsafe extern "C" fn(RawStream) -> i32,
    request_stop: unsafe extern "C" fn(RawStream) -> i32,
    get_sample_rate: unsafe extern "C" fn(RawStream) -> i32,
    get_channel_count: unsafe extern "C" fn(RawStream) -> i32,
    // Android 9 and later; the stream still opens without them.
    set_usage: Option<unsafe extern "C" fn(Builder, i32)>,
    set_input_preset: Option<unsafe extern "C" fn(Builder, i32)>,
}

fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(load).as_ref().map_err(Clone::clone)
}

use std::sync::OnceLock;

fn load() -> Result<Api, String> {
    // SAFETY: libaaudio runs no code of its own when it loads.
    let lib = unsafe { Library::new("libaaudio.so") }.map_err(|e| format!("AAudio not found: {e}"))?;
    macro_rules! sym {
        ($name:literal) => {
            *unsafe { lib.get(concat!($name, "\0").as_bytes()) }.map_err(|e| format!("{}: {e}", $name))?
        };
    }
    macro_rules! optional {
        ($name:literal) => {
            unsafe { lib.get(concat!($name, "\0").as_bytes()) }.ok().map(|s| *s)
        };
    }
    Ok(Api {
        create_builder: sym!("AAudio_createStreamBuilder"),
        builder_delete: sym!("AAudioStreamBuilder_delete"),
        set_direction: sym!("AAudioStreamBuilder_setDirection"),
        set_sharing_mode: sym!("AAudioStreamBuilder_setSharingMode"),
        set_sample_rate: sym!("AAudioStreamBuilder_setSampleRate"),
        set_channel_count: sym!("AAudioStreamBuilder_setChannelCount"),
        set_format: sym!("AAudioStreamBuilder_setFormat"),
        set_performance_mode: sym!("AAudioStreamBuilder_setPerformanceMode"),
        set_data_callback: sym!("AAudioStreamBuilder_setDataCallback"),
        set_error_callback: sym!("AAudioStreamBuilder_setErrorCallback"),
        open_stream: sym!("AAudioStreamBuilder_openStream"),
        stream_close: sym!("AAudioStream_close"),
        request_start: sym!("AAudioStream_requestStart"),
        request_stop: sym!("AAudioStream_requestStop"),
        get_sample_rate: sym!("AAudioStream_getSampleRate"),
        get_channel_count: sym!("AAudioStream_getChannelCount"),
        set_usage: optional!("AAudioStreamBuilder_setUsage"),
        set_input_preset: optional!("AAudioStreamBuilder_setInputPreset"),
        _lib: lib,
    })
}

/// What a stream's callbacks work with; boxed, and freed after the stream is closed.
enum Work {
    Speakers { mixer: Arc<Mutex<Mixer>>, rate: u32, channels: usize },
    Microphone { mic: Arc<MicQueue>, resampler: Option<Resampler>, mono: Vec<f32>, channels: usize },
}

struct Context {
    work: Work,
    /// Set by the error callback (a headset plugged in, the device went away): the audio thread reopens.
    lost: Arc<AtomicBool>,
}

unsafe extern "C" fn on_data(_stream: RawStream, user: *mut c_void, audio: *mut c_void, frames: i32) -> i32 {
    // A panic must not unwind into AAudio's thread.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: `user` is the Context the stream was opened with, alive until the stream is closed.
        let cx = unsafe { &mut *(user as *mut Context) };
        let frames = frames.max(0) as usize;
        match &mut cx.work {
            Work::Speakers { mixer, rate, channels } => {
                // SAFETY: AAudio hands over `frames` frames of `channels` floats.
                let data = unsafe { std::slice::from_raw_parts_mut(audio as *mut f32, frames * *channels) };
                match mixer.lock() {
                    Ok(mut m) => {
                        for frame in data.chunks_mut(*channels) {
                            let [l, r] = m.sample(*rate);
                            match frame.len() {
                                1 => frame[0] = (l + r) * 0.5,
                                _ => {
                                    frame[0] = l;
                                    frame[1] = r;
                                    for s in &mut frame[2..] {
                                        *s = 0.0;
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => data.fill(0.0),
                }
            }
            Work::Microphone { mic, resampler, mono, channels } => {
                // SAFETY: as above, for a microphone.
                let data = unsafe { std::slice::from_raw_parts(audio as *const f32, frames * *channels) };
                mono.clear();
                for frame in data.chunks(*channels) {
                    let mean = frame.iter().sum::<f32>() / frame.len() as f32;
                    match resampler {
                        Some(r) => r.push(mean, mono),
                        None => mono.push(mean),
                    }
                }
                mic.push(mono);
            }
        }
    }));
    AAUDIO_CALLBACK_RESULT_CONTINUE
}

unsafe extern "C" fn on_error(_stream: RawStream, user: *mut c_void, _error: i32) {
    // SAFETY: as in `on_data`. Closing the stream here would deadlock; the audio thread does it.
    let cx = unsafe { &*(user as *const Context) };
    cx.lost.store(true, Ordering::Relaxed);
}

/// An open stream, stopped and closed when dropped.
struct Open {
    api: &'static Api,
    stream: RawStream,
    context: *mut Context,
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: the stream is ours; its callbacks have stopped when `close` returns.
        unsafe {
            (self.api.request_stop)(self.stream);
            (self.api.stream_close)(self.stream);
            drop(Box::from_raw(self.context));
        }
    }
}

fn describe(code: i32) -> String {
    match code {
        -899 => "disconnected".into(),
        -898 => "illegal argument".into(),
        -896 => "internal error".into(),
        -895 => "invalid state".into(),
        -889 => "unavailable (busy, or no permission)".into(),
        -888 => "no free stream".into(),
        -887 => "out of memory".into(),
        -883 => "invalid format".into(),
        -881 => "no audio service".into(),
        -880 => "invalid rate".into(),
        _ => format!("error {code}"),
    }
}

fn open(direction: i32, channels: i32, work: impl FnOnce(u32, usize) -> Work, lost: Arc<AtomicBool>) -> Result<(Open, u32), String> {
    let api = api()?;
    let mut builder: Builder = std::ptr::null_mut();
    // SAFETY: plain AAudio calls on the builder this function makes and deletes.
    unsafe {
        let r = (api.create_builder)(&mut builder);
        if r != 0 || builder.is_null() {
            return Err(describe(r));
        }
        (api.set_direction)(builder, direction);
        (api.set_sharing_mode)(builder, AAUDIO_SHARING_MODE_SHARED);
        (api.set_sample_rate)(builder, SAMPLE_RATE as i32);
        (api.set_channel_count)(builder, channels);
        (api.set_format)(builder, AAUDIO_FORMAT_PCM_FLOAT);
        (api.set_performance_mode)(builder, AAUDIO_PERFORMANCE_MODE_LOW_LATENCY);
        if direction == AAUDIO_DIRECTION_INPUT {
            if let Some(f) = api.set_input_preset {
                f(builder, AAUDIO_INPUT_PRESET_VOICE_COMMUNICATION);
            }
        } else if let Some(f) = api.set_usage {
            f(builder, AAUDIO_USAGE_GAME);
        }
    }
    // The callbacks' context needs the stream's real rate and channels, which are known once it is
    // open: it starts as a placeholder and is filled in before the stream starts.
    let context = Box::into_raw(Box::new(Context {
        work: Work::Speakers { mixer: Arc::new(Mutex::new(Mixer::default())), rate: SAMPLE_RATE, channels: 1 },
        lost,
    }));
    let mut stream: RawStream = std::ptr::null_mut();
    let opened = unsafe {
        (api.set_data_callback)(builder, on_data, context as *mut c_void);
        (api.set_error_callback)(builder, on_error, context as *mut c_void);
        let r = (api.open_stream)(builder, &mut stream);
        (api.builder_delete)(builder);
        r
    };
    if opened != 0 || stream.is_null() {
        // SAFETY: never handed to a stream.
        unsafe { drop(Box::from_raw(context)) };
        return Err(describe(opened));
    }
    // The stream is open but not started: nothing calls back yet.
    let (rate, got) = unsafe { ((api.get_sample_rate)(stream) as u32, (api.get_channel_count)(stream).max(1) as usize) };
    unsafe { (*context).work = work(rate, got) };
    let started = unsafe { (api.request_start)(stream) };
    let open = Open { api, stream, context };
    if started != 0 {
        return Err(describe(started));
    }
    Ok((open, rate))
}

fn open_speakers(mixer: &Arc<Mutex<Mixer>>, lost: &Arc<AtomicBool>) -> Result<(Open, String), String> {
    let mixer = mixer.clone();
    let (open, rate) = open(
        AAUDIO_DIRECTION_OUTPUT,
        2,
        move |rate, channels| Work::Speakers { mixer, rate, channels },
        lost.clone(),
    )?;
    Ok((open, format!("speakers (AAudio, {rate} Hz)")))
}

fn open_microphone(mic: &Arc<MicQueue>, lost: &Arc<AtomicBool>) -> Result<(Open, String), String> {
    let mic = mic.clone();
    let (open, rate) = open(
        AAUDIO_DIRECTION_INPUT,
        1,
        move |rate, channels| Work::Microphone {
            mic,
            resampler: (rate != SAMPLE_RATE).then(|| Resampler::new(rate)),
            mono: Vec::new(),
            channels,
        },
        lost.clone(),
    )?;
    Ok((open, format!("microphone (AAudio, {rate} Hz)")))
}

/// Opens both streams, reopens them when a device goes away (a headset, Bluetooth), and retries
/// a microphone that would not open (the permission may have just been given) every few seconds.
pub fn run(mixer: Arc<Mutex<Mixer>>, mic: Arc<MicQueue>, devices: Arc<Mutex<Devices>>, flag: Arc<AtomicBool>) {
    let lost = Arc::new(AtomicBool::new(false));
    let mut output: Option<Open> = None;
    let mut input: Option<Open> = None;
    let mut retry_in = 0u32;
    while !flag.load(Ordering::Relaxed) {
        if lost.swap(false, Ordering::Relaxed) {
            drop(input.take());
            drop(output.take());
            std::thread::sleep(Duration::from_millis(300));
            retry_in = 0;
        }
        if (output.is_none() || input.is_none()) && retry_in == 0 {
            let mut info = devices.lock().unwrap().clone();
            if output.is_none() {
                match open_speakers(&mixer, &lost) {
                    Ok((s, name)) => (output, info.output) = (Some(s), Ok(name)),
                    Err(e) => info.output = Err(format!("speakers: {e}")),
                }
            }
            if input.is_none() {
                match open_microphone(&mic, &lost) {
                    Ok((s, name)) => (input, info.input) = (Some(s), Ok(name)),
                    Err(e) => {
                        info.input =
                            Err(format!("microphone: {e}; allow Microphone for Horizon in Android's app settings"))
                    }
                }
            }
            *devices.lock().unwrap() = info;
            retry_in = 100;
        }
        std::thread::sleep(Duration::from_millis(50));
        retry_in = retry_in.saturating_sub(1);
    }
    drop(input);
    drop(output);
}
