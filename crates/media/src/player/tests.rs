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
    let st = wait_for(&p, 5.0, "metadata", |s| s.ready >= 1);
    assert!((st.duration - 1.0).abs() < 0.01, "{st:?}");
    assert!(st.has_audio && !st.has_video);
    assert_eq!(st.audio_codec, "pcm");
    wait_for(&p, 5.0, "enough data", |s| s.ready == 4);
    p.play();
    let st = wait_for(&p, 5.0, "time to advance", |s| s.time > 0.2);
    assert!(st.time < 0.9);
    // Seek forward; time jumps and keeps going to the end.
    p.seek(0.7);
    let st = wait_for(&p, 5.0, "seek", |s| !s.seeking);
    assert!(st.time >= 0.69, "{st:?}");
    let st = wait_for(&p, 5.0, "end", |s| s.ended);
    assert!((st.time - 1.0).abs() < 0.02, "{st:?}");
    assert!(p.frames_played() > 0);
}

#[test]
fn pause_freezes_time_and_rate_speeds_up() {
    null_env();
    let wav = crate::wav::encode(48_000, 1, &crate::wav::tone(48_000, 1, 220.0, 3.0, 0.3));
    let p = Player::new(PlayerSource::Bytes { data: wav, mime: "audio/wav".into() });
    wait_for(&p, 5.0, "data", |s| s.ready == 4);
    p.set_rate(2.0);
    p.play();
    std::thread::sleep(Duration::from_millis(400));
    p.pause();
    let t = p.status().time;
    assert!(t > 0.5, "double speed should cover > 0.5 s in 0.4 s, got {t}");
    std::thread::sleep(Duration::from_millis(150));
    assert!((p.status().time - t).abs() < 1e-9);
}

#[test]
fn unsupported_bytes_report_error() {
    null_env();
    let p = Player::new(PlayerSource::Bytes { data: b"this is not media at all, just text".repeat(20), mime: "text/plain".into() });
    let st = wait_for(&p, 5.0, "error", |s| s.error.is_some());
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
    let st = wait_for(&p, 5.0, "ready", |s| s.ready >= 3);
    assert!(st.has_audio);
    assert_eq!(st.audio_codec, "opus");
    p.play();
    // Plays the first second, then stalls waiting for data.
    let st = wait_for(&p, 5.0, "advance", |s| s.time > 0.5);
    assert!(st.time <= 1.0);
    std::thread::sleep(Duration::from_millis(800));
    let st = p.status();
    assert!(st.time <= 1.0 + 1e-6 && st.ready <= 2, "{st:?}");
    // Append the second second and end the stream: playback finishes.
    {
        let mut m = h.lock();
        m.buffers[0].append(&wb::cluster(1000, &blocks)).unwrap();
        m.ended = true;
    }
    let st = wait_for(&p, 5.0, "ended", |s| s.ended);
    assert!((st.time - 2.0).abs() < 0.05, "{st:?}");
    assert!(st.decoded_frames >= 96_000 - 312, "{st:?}");
}
