//! Audio decoders: Symphonia's (MP3, AAC-LC, FLAC, Vorbis, ALAC, PCM,
//! ADPCM) and `opus-decoder` for Opus, behind one interface producing
//! interleaved `f32` PCM.

use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::packet::Packet;
use symphonia::core::units::{Duration as SymDuration, Timestamp};

/// Decoded audio.
#[derive(Clone, Debug, Default)]
pub struct Pcm {
    pub rate: u32,
    pub channels: usize,
    /// Interleaved samples.
    pub samples: Vec<f32>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels
        }
    }

    /// Drop the first `n` frames.
    pub fn skip_frames(&mut self, n: usize) {
        let n = (n * self.channels).min(self.samples.len());
        self.samples.drain(..n);
    }
}

enum Inner {
    Symphonia(Box<dyn AudioDecoder>),
    Opus(OpusState),
}

struct OpusState {
    single: Option<opus_decoder::OpusDecoder>,
    multi: Option<opus_decoder::OpusMultistreamDecoder>,
    channels: usize,
    pre_skip: usize,
    /// Pre-skip samples still to drop (start of stream).
    skip_left: usize,
    /// Output gain (linear).
    gain: f32,
    buf: Vec<f32>,
}

/// An audio decoder for one track.
pub struct Decoder {
    inner: Inner,
}

/// Short codec name for diagnostics ("opus", "aac", "mp3", ...).
pub fn codec_name(params: &AudioCodecParameters) -> &'static str {
    use symphonia::core::codecs::audio::well_known::*;
    match params.codec {
        c if c == CODEC_ID_OPUS => "opus",
        c if c == CODEC_ID_AAC => "aac",
        c if c == CODEC_ID_MP3 => "mp3",
        c if c == CODEC_ID_MP2 => "mp2",
        c if c == CODEC_ID_MP1 => "mp1",
        c if c == CODEC_ID_VORBIS => "vorbis",
        c if c == CODEC_ID_FLAC => "flac",
        c if c == CODEC_ID_ALAC => "alac",
        _ => "pcm",
    }
}

impl Decoder {
    pub fn new(params: &AudioCodecParameters) -> Result<Decoder, String> {
        if params.codec == CODEC_ID_OPUS {
            return Ok(Decoder { inner: Inner::Opus(OpusState::new(params)?) });
        }
        let opts = AudioDecoderOptions::default();
        let dec = symphonia::default::get_codecs().make_audio_decoder(params, &opts).map_err(|e| format!("unsupported audio codec: {e}"))?;
        Ok(Decoder { inner: Inner::Symphonia(dec) })
    }

    /// Decode one packet (one container frame / block).
    pub fn decode(&mut self, data: &[u8]) -> Result<Pcm, String> {
        match &mut self.inner {
            Inner::Symphonia(dec) => {
                let packet = Packet::new(0, Timestamp::new(0), SymDuration::new(0), data.to_vec());
                match dec.decode(&packet) {
                    Ok(buf) => {
                        let spec = buf.spec().clone();
                        let mut samples: Vec<f32> = Vec::new();
                        buf.copy_to_vec_interleaved::<f32>(&mut samples);
                        Ok(Pcm { rate: spec.rate(), channels: spec.channels().count(), samples })
                    }
                    Err(symphonia::core::errors::Error::DecodeError(e)) => {
                        // A corrupt frame: skip it like a browser would.
                        Err(format!("decode error: {e}"))
                    }
                    Err(e) => Err(e.to_string()),
                }
            }
            Inner::Opus(o) => o.decode(data),
        }
    }

    /// Forget decoder state (after a seek). `at_start`: the next packet is
    /// the first of the stream (Opus pre-skip applies again).
    pub fn reset(&mut self, at_start: bool) {
        match &mut self.inner {
            Inner::Symphonia(dec) => dec.reset(),
            Inner::Opus(o) => {
                if let Some(d) = o.single.as_mut() {
                    d.reset();
                }
                if let Some(d) = o.multi.as_mut() {
                    d.reset();
                }
                o.skip_left = if at_start { o.pre_skip } else { 0 };
            }
        }
    }
}

impl OpusState {
    fn new(params: &AudioCodecParameters) -> Result<OpusState, String> {
        let head = params.extra_data.as_deref().unwrap_or(&[]);
        let mut channels = params.channels.as_ref().map(|c| c.count()).unwrap_or(2).max(1);
        let mut pre_skip = 0usize;
        let mut gain = 1.0f32;
        let mut family = 0u8;
        let mut streams = 1usize;
        let mut coupled = if channels > 1 { 1 } else { 0 };
        let mut mapping: Vec<u8> = (0..channels as u8).collect();
        if head.len() >= 19 && &head[..8] == b"OpusHead" {
            channels = head[9].max(1) as usize;
            pre_skip = u16::from_le_bytes([head[10], head[11]]) as usize;
            let g = i16::from_le_bytes([head[16], head[17]]);
            gain = 10f32.powf(g as f32 / (20.0 * 256.0));
            family = head[18];
            if family != 0 && head.len() >= 21 + channels {
                streams = head[19] as usize;
                coupled = head[20] as usize;
                mapping = head[21..21 + channels].to_vec();
            } else {
                coupled = if channels > 1 { 1 } else { 0 };
                mapping = (0..channels as u8).collect();
            }
        }
        let (single, multi) = if family == 0 && channels <= 2 {
            (Some(opus_decoder::OpusDecoder::new(48_000, channels).map_err(|e| format!("opus: {e:?}"))?), None)
        } else {
            (None, Some(opus_decoder::OpusMultistreamDecoder::new(48_000, channels, streams, coupled, &mapping).map_err(|e| format!("opus: {e:?}"))?))
        };
        Ok(OpusState { single, multi, channels, pre_skip, skip_left: pre_skip, gain, buf: vec![0.0; 5760 * channels] })
    }

    fn decode(&mut self, data: &[u8]) -> Result<Pcm, String> {
        if self.buf.len() < 5760 * self.channels {
            self.buf.resize(5760 * self.channels, 0.0);
        }
        let n = if let Some(d) = self.single.as_mut() {
            d.decode_float(data, &mut self.buf, false)
        } else if let Some(d) = self.multi.as_mut() {
            d.decode_float(data, &mut self.buf, false)
        } else {
            return Err("opus: no decoder".into());
        }
        .map_err(|e| format!("opus: {e:?}"))?;
        let mut pcm = Pcm { rate: 48_000, channels: self.channels, samples: self.buf[..n * self.channels].to_vec() };
        if (self.gain - 1.0).abs() > 1e-6 {
            for s in &mut pcm.samples {
                *s *= self.gain;
            }
        }
        if self.skip_left > 0 {
            let k = self.skip_left.min(n);
            pcm.skip_frames(k);
            self.skip_left -= k;
        }
        Ok(pcm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_head_parsing_and_silence_frame() {
        // OpusHead: version 1, 2 channels, pre-skip 312, 48 kHz, gain 0, family 0.
        let mut head = b"OpusHead".to_vec();
        head.push(1);
        head.push(2);
        head.extend_from_slice(&312u16.to_le_bytes());
        head.extend_from_slice(&48_000u32.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes());
        head.push(0);
        let mut p = AudioCodecParameters::new();
        p.for_codec(CODEC_ID_OPUS).with_extra_data(head.into_boxed_slice());
        let mut d = Decoder::new(&p).unwrap();
        // A CELT-only 20 ms fullband stereo frame with no payload (TOC
        // config 31, stereo, one frame): decodes to silence.
        let toc = (31u8 << 3) | 0b100;
        let pcm = d.decode(&[toc]).unwrap();
        assert_eq!(pcm.rate, 48_000);
        assert_eq!(pcm.channels, 2);
        // 960 frames minus the 312 pre-skip.
        assert_eq!(pcm.frames(), 960 - 312);
        let pcm = d.decode(&[toc]).unwrap();
        assert_eq!(pcm.frames(), 960);
    }
}
