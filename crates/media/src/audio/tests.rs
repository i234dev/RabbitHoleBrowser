use super::*;

fn null_env() {
    // Tests never touch a real device.
    std::env::set_var("RH_AUDIO", "null");
}

#[test]
fn stereo_conversion() {
    assert_eq!(to_stereo(&[0.5, -0.5], 1), vec![[0.5, 0.5], [-0.5, -0.5]]);
    assert_eq!(to_stereo(&[0.1, 0.2, 0.3, 0.4], 2), vec![[0.1, 0.2], [0.3, 0.4]]);
    let six = to_stereo(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0], 6);
    assert!(six[0][0] > 0.3 && six[0][1] == 0.0);
}

#[test]
fn linear_resample_lengths() {
    let f = vec![[0.0f32, 0.0]; 44100];
    assert_eq!(resample_linear(&f, 44100, 48000).len(), 48000);
    assert_eq!(resample_linear(&f, 44100, 22050).len(), 22050);
}

fn state() -> SinkState {
    SinkState {
        queue: VecDeque::new(),
        queue_start: 0.0,
        pos: 0.0,
        rate: 0,
        playing: true,
        volume: 1.0,
        muted: false,
        speed: 1.0,
        last_gain: 1.0,
        eos: false,
        clock: 0.0,
        floor: 0.0,
        underrun: false,
        frames_mixed: 0,
    }
}

#[test]
fn mixing_advances_clock_and_resamples() {
    let mut st = state();
    st.rate = 24_000;
    st.queue.extend(std::iter::repeat_n([0.5f32, 0.5], 24_000));
    let mut out = vec![[0.0f32; 2]; 4800];
    mix_sink(&mut st, &mut out, 48_000, 0.0);
    // 4800 output frames at 48 kHz = 0.1 s = 2400 source frames.
    assert!((st.clock - 0.1).abs() < 1e-3, "clock {}", st.clock);
    assert!(out.iter().all(|f| (f[0] - 0.5).abs() < 1e-3));
    assert!(!st.underrun);
    // Double speed consumes twice as fast.
    st.speed = 2.0;
    mix_sink(&mut st, &mut out, 48_000, 0.0);
    assert!((st.clock - 0.3).abs() < 1e-3, "clock {}", st.clock);
}

#[test]
fn mixing_underrun_and_volume() {
    let mut st = state();
    st.rate = 48_000;
    st.volume = 0.5;
    st.last_gain = 0.5;
    st.queue.extend(std::iter::repeat_n([1.0f32, 1.0], 100));
    let mut out = vec![[0.0f32; 2]; 480];
    mix_sink(&mut st, &mut out, 48_000, 0.0);
    assert!(st.underrun);
    assert!((out[10][0] - 0.5).abs() < 1e-6);
    assert_eq!(out[200][0], 0.0);
    // Muted: silence but time advances.
    let mut st = state();
    st.rate = 48_000;
    st.muted = true;
    st.last_gain = 0.0;
    st.queue.extend(std::iter::repeat_n([1.0f32, 1.0], 4800));
    let mut out = vec![[0.0f32; 2]; 480];
    mix_sink(&mut st, &mut out, 48_000, 0.0);
    assert!(out.iter().all(|f| f[0] == 0.0));
    assert!((st.clock - 0.01).abs() < 1e-6);
}

#[test]
fn push_aligns_timestamps() {
    null_env();
    let sink = AudioSink::new();
    sink.flush(10.0);
    // A chunk that starts before the seek target is trimmed.
    sink.push(9.5, 1000, 1, &vec![0.25; 1000]);
    assert!((sink.queued_end() - 10.5).abs() < 1e-6, "end {}", sink.queued_end());
    assert!((sink.buffered_secs() - 0.5).abs() < 1e-3);
    // A gap is filled with silence.
    sink.push(11.0, 1000, 1, &vec![0.25; 500]);
    assert!((sink.queued_end() - 11.5).abs() < 1e-6, "end {}", sink.queued_end());
    // Overlap is dropped.
    sink.push(11.25, 1000, 1, &vec![0.25; 500]);
    assert!((sink.queued_end() - 11.75).abs() < 1e-6, "end {}", sink.queued_end());
}

#[test]
fn null_sink_plays_in_real_time() {
    null_env();
    let sink = AudioSink::new();
    sink.flush(0.0);
    sink.push(0.0, 48_000, 2, &crate::wav::tone(48_000, 2, 440.0, 1.0, 0.5));
    sink.set_eos(true);
    sink.set_playing(true);
    let t0 = std::time::Instant::now();
    std::thread::sleep(Duration::from_millis(300));
    let c = sink.clock();
    let el = t0.elapsed().as_secs_f64();
    assert!(c > 0.1 && c <= el + 0.05, "clock {c} after {el}s");
    sink.set_playing(false);
    std::thread::sleep(Duration::from_millis(50));
    let paused = sink.clock();
    std::thread::sleep(Duration::from_millis(100));
    assert!((sink.clock() - paused).abs() < 1e-9, "clock moved while paused");
    let s = stats();
    assert_eq!(s.backend, "null");
    assert!(s.frames > 0 && s.nonsilent_samples > 0 && s.peak > 0.3, "{s:?}");
}
