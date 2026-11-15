//! The SDL3 playback device: a callback thread pulls audio from the shared mixer.
use crate::mixer::Mixer;
use sdl3::audio::{AudioCallback, AudioFormat, AudioSpec, AudioStream, AudioStreamWithCallback};
use std::sync::{Arc, Mutex};

struct Pull {
    mixer: Arc<Mutex<Mixer>>,
    buf: Vec<f32>,
}

impl AudioCallback<f32> for Pull {
    fn callback(&mut self, stream: &mut AudioStream, requested: i32) {
        self.buf.resize(requested.max(0) as usize & !1, 0.0);
        if let Ok(mut m) = self.mixer.lock() {
            m.mix(&mut self.buf);
        } else {
            self.buf.fill(0.0);
        }
        let _ = stream.put_data_f32(&self.buf);
    }
}

/// The running audio output. Keep it alive while the game runs; talk to `mixer` from the game thread.
pub struct AudioEngine {
    pub mixer: Arc<Mutex<Mixer>>,
    _stream: AudioStreamWithCallback<Pull>,
}

impl AudioEngine {
    /// The lowest output rate used (CD quality; the original's highest).
    pub const MIN_RATE: u32 = 44100;
    /// The output rate when the system's cannot be read.
    pub const DEFAULT_RATE: u32 = 48000;

    /// The rate to open the output at: `wanted` (0: the system's playback rate, `DEFAULT_RATE` when unknown), never
    /// below `MIN_RATE`.
    pub fn output_rate(audio: &sdl3::AudioSubsystem, wanted: u32) -> u32 {
        let r = if wanted != 0 {
            wanted
        } else {
            audio.default_playback_device().format().ok().and_then(|(spec, _)| spec.freq).filter(|&f| f > 0).map(|f| f as u32).unwrap_or(Self::DEFAULT_RATE)
        };
        if r < Self::MIN_RATE {
            Self::DEFAULT_RATE
        } else {
            r
        }
    }

    /// Opens the playback device at `rate` Hz (see `output_rate`), stereo float.
    pub fn start(audio: &sdl3::AudioSubsystem, rate: u32) -> Result<AudioEngine, String> {
        let mixer = Arc::new(Mutex::new(Mixer::new(rate)));
        let spec = AudioSpec { freq: Some(rate as i32), channels: Some(2), format: Some(AudioFormat::f32_sys()) };
        let stream = audio.open_playback_stream(&spec, Pull { mixer: mixer.clone(), buf: Vec::new() }).map_err(|e| e.to_string())?;
        stream.resume().map_err(|e| e.to_string())?;
        Ok(AudioEngine { mixer, _stream: stream })
    }
}
