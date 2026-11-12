use super::*;
use crate::demux::webm::build as wb;
use crate::mse::SourceBufferState;

fn null_env() {
    std::env::set_var("RH_AUDIO", "null");
}

fn wait_for(p: &Player, secs: f64, what: &str, f: impl Fn(&Status) -> bool) -> Status {
    let t0 = Instant::now();
    loop {
        let st = p.status();
        if f(&st) {
            return st;
        }
        if t0.elapsed().as_secs_f64() > secs {
            panic!("timed out waiting for {what}: {st:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn wav_file_plays_seeks_and_ends() {
    null_env();
    let wav = crate::wav::encode(22_050, 2, &crate::wav::tone(22_050, 2, 440.0, 1.0, 0.5));
    let p = Player::new(PlayerSource::Bytes { data: wav, mime: "audio/wav".into() });
    let st = wait_for(&p, 30.0, "metadata", |s| s.ready >= 1);
    assert!((st.duration - 1.0).abs() < 0.01, "{st:?}");
    assert!(st.has_audio && !st.has_video);
    assert_eq!(st.audio_codec, "pcm");
    wait_for(&p, 30.0, "enough data", |s| s.ready == 4);
    p.play();
    wait_for(&p, 30.0, "time to advance", |s| s.time > 0.2);
    // Seek forward; time jumps and keeps going to the end.
    p.seek(0.7);
    let st = wait_for(&p, 30.0, "seek", |s| !s.seeking);
    assert!(st.time >= 0.69, "{st:?}");
    let st = wait_for(&p, 30.0, "end", |s| s.ended);
    assert!((st.time - 1.0).abs() < 0.02, "{st:?}");
    assert!(p.frames_played() > 0);
}

#[test]
fn pause_freezes_time_and_rate_speeds_up() {
    null_env();
    let wav = crate::wav::encode(48_000, 1, &crate::wav::tone(48_000, 1, 220.0, 3.0, 0.3));
    let p = Player::new(PlayerSource::Bytes { data: wav, mime: "audio/wav".into() });
    wait_for(&p, 30.0, "data", |s| s.ready == 4);
    p.set_rate(2.0);
    p.play();
    wait_for(&p, 30.0, "time to advance", |s| s.time > 0.5);
    p.pause();
    std::thread::sleep(Duration::from_millis(50));
    let t = p.status().time;
    // Double speed: media time = 2 x the output frames the mixer produced
    // (48 kHz source and null device), whatever the machine load.
    let expect = 2.0 * p.frames_played() as f64 / 48_000.0;
    assert!((t - expect).abs() < 0.05, "time {t} vs 2 x mixed {expect}");
    std::thread::sleep(Duration::from_millis(150));
    assert!((p.status().time - t).abs() < 1e-9);
}

#[test]
fn unsupported_bytes_report_error() {
    null_env();
    let p = Player::new(PlayerSource::Bytes { data: b"this is not media at all, just text".repeat(20), mime: "text/plain".into() });
    let st = wait_for(&p, 30.0, "error", |s| s.error.is_some());
    assert_eq!(st.error.unwrap().0, ERR_SRC_NOT_SUPPORTED);
}

#[test]
fn mse_opus_plays_buffered_range() {
    null_env();
    let h = MseHandle::new();
    let mut sb = SourceBufferState::new("audio/webm; codecs=\"opus\"").unwrap();
    sb.append(&wb::audio_init("A_OPUS", b"OpusHead\x01\x02\x38\x01\x80\xbb\0\0\0\0\0", 48000.0, 2, None)).unwrap();
    let toc = [(31u8 << 3) | 0b100];
    let blocks: Vec<(i16, &[u8])> = (0..50).map(|i| ((i * 20) as i16, &toc[..])).collect();
    sb.append(&wb::cluster(0, &blocks)).unwrap();
    {
        let mut m = h.lock();
        m.duration = 2.0;
        m.buffers.push(sb);
    }
    let p = Player::new(PlayerSource::Mse(h.clone()));
    // Generous timeouts and condition waits only: under heavy load the
    // pipeline is slower, never different.
    let st = wait_for(&p, 30.0, "ready", |s| s.ready >= 3);
    assert!(st.has_audio);
    assert_eq!(st.audio_codec, "opus");
    p.play();
    // Plays the first second, then stalls at the end of the buffered range
    // (readyState drops once the worker sees it).
    let st = wait_for(&p, 30.0, "stall at the buffered end", |s| s.time >= 0.98 && s.ready <= 2);
    assert!(st.time <= 1.0 + 1e-6, "{st:?}");
    // It stays there while no data arrives.
    std::thread::sleep(Duration::from_millis(200));
    let st = p.status();
    assert!(st.time <= 1.0 + 1e-6, "{st:?}");
    // Append the second second and end the stream: playback finishes.
    {
        let mut m = h.lock();
        m.buffers[0].append(&wb::cluster(1000, &blocks)).unwrap();
        m.ended = true;
    }
    let st = wait_for(&p, 30.0, "ended", |s| s.ended);
    assert!((st.time - 2.0).abs() < 0.05, "{st:?}");
    assert!(st.decoded_frames >= 96_000 - 312, "{st:?}");
}

#[test]
fn av1_key_frame_detection() {
    // Temporal delimiter + sequence header: a key frame.
    assert!(av1_is_key(&[0x12, 0x00, 0x0A, 0x01, 0x00]));
    // Temporal delimiter + frame OBU with frame_type INTER.
    assert!(!av1_is_key(&[0x12, 0x00, 0x32, 0x01, 0b0010_0000]));
    // Frame OBU with frame_type KEY.
    assert!(av1_is_key(&[0x32, 0x01, 0b0000_0000]));
}

#[test]
fn av1_decoder_opens() {
    let mut d = crate::video::Av1Decoder::new().expect("rav1d context");
    // Garbage in: no pictures, no crash.
    assert!(d.decode(&[0x12, 0x00, 0xff, 0xff], 0.0).is_empty());
    d.flush();
}
