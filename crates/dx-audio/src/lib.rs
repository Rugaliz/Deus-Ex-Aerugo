//! Audio for the port: decoding of the game's sound formats, the port of Galaxy.dll's software mixer with its effect
//! voices and tracker engine, and an SDL3 output device.
//! The mixer itself has no device dependency: `Mixer::mix` fills a buffer, so it can be tested and rendered to a file.

pub mod decode;
pub mod galaxy;
pub mod device;
pub mod mixer;
pub mod phoneme;

pub use decode::{encode_wav, PcmSound};
pub use device::AudioEngine;
pub use mixer::{Mixer, VoiceData};
pub use phoneme::PhonemeDetector;
