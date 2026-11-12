//! Media playback for RabbitHole: audio output (one mixer thread per
//! process over WASAPI / PulseAudio / ALSA / CoreAudio or a null sink),
//! container demuxing, audio decoding (Symphonia for MP3 / AAC / FLAC /
//! Vorbis / PCM / ALAC, `opus-decoder` for Opus) and the playback pipeline
//! behind `HTMLMediaElement` and Media Source Extensions.

pub mod audio;
pub mod decode;
pub mod demux;
pub mod mse;
pub mod oneshot;
pub mod player;
pub mod source;
pub mod video;
pub mod wav;

pub use audio::{stats as audio_stats, AudioSink, AudioStats};
