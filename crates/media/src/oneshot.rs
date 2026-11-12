//! Whole-file decoding (Web Audio `decodeAudioData`): every audio frame of
//! a file in memory, planar, resampled to a target rate.

use crate::decode::Decoder;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

/// Decoded audio: one Vec per channel at `rate`.
pub struct DecodedAudio {
    pub rate: u32,
    pub channels: Vec<Vec<f32>>,
}

/// Decode `bytes` (any supported container / codec) to planar float PCM.
/// `target_rate` 0 keeps the file's rate.
pub fn decode_all(bytes: Vec<u8>, target_rate: u32) -> Result<DecodedAudio, String> {
    let src = crate::source::RangedSource::from_bytes(bytes, "");
    let mss = MediaSourceStream::new(Box::new(src.reader()), MediaSourceStreamOptions::default());
    let mut format = symphonia::default::get_probe().probe(&Hint::new(), mss, FormatOptions::default(), MetadataOptions::default()).map_err(|e| format!("Unable to decode audio data: {e}"))?;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.as_ref().and_then(|p| p.audio()).is_some())
        .cloned()
        .ok_or("Unable to decode audio data: no audio track")?;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).cloned().ok_or("no audio parameters")?;
    let mut dec = Decoder::new(&params)?;
    let mut rate = 0u32;
    let mut channels: Vec<Vec<f32>> = Vec::new();
    loop {
        let p = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(symphonia::core::errors::Error::IoError(_)) => break,
            Err(symphonia::core::errors::Error::ResetRequired) => {
                dec = Decoder::new(&params)?;
                continue;
            }
            Err(e) => {
                if channels.is_empty() {
                    return Err(format!("Unable to decode audio data: {e}"));
                }
                break;
            }
        };
        if p.track_id != track.id {
            continue;
        }
        let Ok(pcm) = dec.decode(&p.data) else { continue };
        if pcm.channels == 0 {
            continue;
        }
        if channels.is_empty() {
            channels = vec![Vec::new(); pcm.channels];
            rate = pcm.rate;
        }
        let n = channels.len();
        for (i, s) in pcm.samples.iter().enumerate() {
            let c = i % pcm.channels;
            if c < n {
                channels[c].push(*s);
            }
        }
    }
    if channels.is_empty() || rate == 0 {
        return Err("Unable to decode audio data: no samples".into());
    }
    if target_rate != 0 && target_rate != rate {
        for ch in &mut channels {
            *ch = resample_mono(ch, rate, target_rate);
        }
        rate = target_rate;
    }
    Ok(DecodedAudio { rate, channels })
}

/// Linear-interpolation resampling of one channel.
pub fn resample_mono(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() {
        return x.to_vec();
    }
    let step = from as f64 / to as f64;
    let n = ((x.len() as f64) / step).round() as usize;
    (0..n)
        .map(|i| {
            let p = i as f64 * step;
            let k = p.floor() as usize;
            let f = (p - k as f64) as f32;
            let a = x[k.min(x.len() - 1)];
            let b = x[(k + 1).min(x.len() - 1)];
            a + (b - a) * f
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_wav_and_resamples() {
        let wav = crate::wav::encode(22_050, 2, &crate::wav::tone(22_050, 2, 440.0, 0.5, 0.5));
        let d = decode_all(wav.clone(), 0).unwrap();
        assert_eq!(d.rate, 22_050);
        assert_eq!(d.channels.len(), 2);
        assert_eq!(d.channels[0].len(), 11_025);
        let d = decode_all(wav, 44_100).unwrap();
        assert_eq!(d.channels[0].len(), 22_050);
        assert!(decode_all(b"nope".to_vec(), 0).is_err());
    }
}
