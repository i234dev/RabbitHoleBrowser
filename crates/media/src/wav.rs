//! Minimal WAV writing (output dumps and generated test fixtures).

/// A 44-byte header for 16-bit PCM with `data_bytes` of sample data.
pub fn header(rate: u32, channels: u16, data_bytes: u32) -> [u8; 44] {
    let mut h = [0u8; 44];
    let block = channels as u32 * 2;
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&channels.to_le_bytes());
    h[24..28].copy_from_slice(&rate.to_le_bytes());
    h[28..32].copy_from_slice(&(rate * block).to_le_bytes());
    h[32..34].copy_from_slice(&(block as u16).to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    h
}

/// A complete 16-bit PCM WAV file from interleaved float samples.
pub fn encode(rate: u32, channels: u16, samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(&header(rate, channels, (samples.len() * 2) as u32));
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    out
}

/// A sine tone (interleaved, same signal on every channel).
pub fn tone(rate: u32, channels: u16, freq: f32, secs: f32, amp: f32) -> Vec<f32> {
    let n = (rate as f32 * secs) as usize;
    let mut v = Vec::with_capacity(n * channels as usize);
    for i in 0..n {
        let s = amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin();
        for _ in 0..channels {
            v.push(s);
        }
    }
    v
}
