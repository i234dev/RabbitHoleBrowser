//! Decoding of small generated fixtures (0.5 s 440 Hz tones and a red
//! 64×48 AV1 clip, made with ffmpeg from synthetic sources: no
//! third-party content) through every container / codec path.

use rabbithole_media::demux::{ParseEvent, TrackKind};
use rabbithole_media::mse::SourceBufferState;
use rabbithole_media::oneshot::decode_all;
use rabbithole_media::player::{Player, PlayerSource};
use std::time::{Duration, Instant};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// Dominant frequency estimate from zero crossings.
fn frequency(x: &[f32], rate: u32) -> f32 {
    let skip = x.len() / 10;
    let x = &x[skip..x.len() - skip];
    let crossings = x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    crossings as f32 * rate as f32 / x.len() as f32
}

#[test]
fn every_audio_format_decodes_to_the_tone() {
    for name in ["tone.mp3", "tone.ogg", "tone.opus", "tone.webm", "tone.flac", "tone.m4a", "tone-frag.m4a"] {
        let d = decode_all(fixture(name), 48_000).unwrap_or_else(|e| panic!("{name}: {e}"));
        let secs = d.channels[0].len() as f32 / d.rate as f32;
        assert!((0.4..0.65).contains(&secs), "{name}: {secs} s");
        let f = frequency(&d.channels[0], d.rate);
        assert!((f - 440.0).abs() < 15.0, "{name}: {f} Hz");
        let peak = d.channels[0].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.05, "{name}: peak {peak}");
    }
}

#[test]
fn files_play_through_the_player() {
    std::env::set_var("RH_AUDIO", "null");
    for (name, mime) in [("tone.mp3", "audio/mpeg"), ("tone.webm", "audio/webm"), ("tone.m4a", "audio/mp4"), ("tone.ogg", "audio/ogg")] {
        let p = Player::new(PlayerSource::Bytes { data: fixture(name), mime: mime.into() });
        let t0 = Instant::now();
        while p.status().ready < 4 {
            assert!(t0.elapsed() < Duration::from_secs(10), "{name}: {:?}", p.status());
            std::thread::sleep(Duration::from_millis(10));
        }
        let st = p.status();
        assert!((st.duration - 0.5).abs() < 0.1, "{name}: {st:?}");
        p.play();
        while !p.status().ended {
            assert!(t0.elapsed() < Duration::from_secs(10), "{name}: {:?}", p.status());
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(p.status().decoded_frames > 20_000, "{name}: {:?}", p.status());
    }
}

#[test]
fn mse_parses_real_webm_and_fragmented_mp4() {
    for (name, mime, codec) in [("tone.webm", "audio/webm; codecs=\"opus\"", "opus"), ("tone-frag.m4a", "audio/mp4; codecs=\"mp4a.40.2\"", "aac"), ("red-av1-frag.mp4", "video/mp4; codecs=\"av01.0.00M.08\"", "av1"), ("red-av1.webm", "video/webm; codecs=\"av01.0.00M.08\"", "av1")] {
        let mut sb = SourceBufferState::new(mime).unwrap();
        let bytes = fixture(name);
        // Append in odd-sized pieces.
        let mut inits = Vec::new();
        for c in bytes.chunks(333) {
            inits.extend(sb.append(c).unwrap_or_else(|e| panic!("{name}: {e}")).inits);
        }
        assert_eq!(inits.len(), 1, "{name}");
        assert_eq!(inits[0].0[0].codec, codec, "{name}");
        let b = sb.buffered();
        assert_eq!(b.len(), 1, "{name}: {b:?}");
        assert!(b[0].0 < 0.05 && (b[0].1 - 0.5).abs() < 0.1, "{name}: {b:?}");
        let kind = sb.tracks[0].info.kind;
        assert_eq!(kind, if codec == "av1" { TrackKind::Video } else { TrackKind::Audio });
        let _ = ParseEvent::Init { tracks: Vec::new(), duration: None };
    }
}

#[test]
fn av1_frames_decode_to_red() {
    let mut sb = SourceBufferState::new("video/webm").unwrap();
    sb.append(&fixture("red-av1.webm")).unwrap();
    let frames = &sb.tracks[0].frames;
    assert!(frames.len() >= 4, "{}", frames.len());
    let mut dec = rabbithole_media::video::Av1Decoder::new().unwrap();
    let mut pics = Vec::new();
    for f in frames {
        pics.extend(dec.decode(&f.data, f.pts));
    }
    assert!(!pics.is_empty());
    let p = &pics[0];
    assert_eq!((p.width, p.height), (64, 48));
    let (r, g, b) = (p.rgba[0], p.rgba[1], p.rgba[2]);
    assert!(r > 200 && g < 60 && b < 60, "pixel {r},{g},{b}");
}

#[test]
fn av1_file_shows_frames_in_the_player() {
    std::env::set_var("RH_AUDIO", "null");
    let p = Player::new(PlayerSource::Bytes { data: fixture("red-av1.webm"), mime: "video/webm".into() });
    let t0 = Instant::now();
    loop {
        if let Some((_, f)) = p.video_frame(0) {
            assert_eq!((f.width, f.height), (64, 48));
            assert!(f.rgba[0] > 200);
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", p.status());
        std::thread::sleep(Duration::from_millis(10));
    }
    let st = p.status();
    assert!(st.has_video && st.video_codec == "av1", "{st:?}");
}
