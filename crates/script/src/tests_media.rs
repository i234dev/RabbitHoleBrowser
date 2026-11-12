//! HTMLMediaElement / MediaSource tests against the real pipeline with
//! the null audio sink.

use crate::tests::{run, setup, MockEnv};
use crate::ScriptHost;
use std::time::{Duration, Instant};

fn null_audio() {
    std::env::set_var("RH_AUDIO", "null");
}

/// Run the event loop in real time until `cond` (a JS expression) is
/// true or `secs` pass; returns the final value of `out` (a JS expression).
fn pump(env: &mut MockEnv, host: &mut ScriptHost, secs: f64, cond: &str, out: &str) -> String {
    let start = Instant::now();
    // Monotonic across calls.
    let base = env.now.max(1000.0);
    loop {
        let now = base + start.elapsed().as_secs_f64() * 1000.0;
        env.now = now;
        host.run_timers(env, now);
        host.run_jobs(env);
        if host.eval_for_console(env, &format!("!!({cond})")) == "true" || start.elapsed().as_secs_f64() > secs {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    host.eval_for_console(env, out)
}

const WAV_JS: &str = r#"
function wav(seconds, rate) {
  const n = Math.floor(seconds * rate);
  const buf = new ArrayBuffer(44 + n * 2);
  const v = new DataView(buf);
  const str = (o, s) => { for (let i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i)); };
  str(0, 'RIFF'); v.setUint32(4, 36 + n * 2, true); str(8, 'WAVE'); str(12, 'fmt ');
  v.setUint32(16, 16, true); v.setUint16(20, 1, true); v.setUint16(22, 1, true); v.setUint32(24, rate, true);
  v.setUint32(28, rate * 2, true); v.setUint16(32, 2, true); v.setUint16(34, 16, true); str(36, 'data'); v.setUint32(40, n * 2, true);
  for (let i = 0; i < n; i++) v.setInt16(44 + i * 2, Math.round(8000 * Math.sin(2 * Math.PI * 440 * i / rate)), true);
  return buf;
}
"#;

const EVENTS: &str = "['loadstart','progress','suspend','abort','error','emptied','stalled','loadedmetadata','loadeddata','canplay','canplaythrough','playing','waiting','seeking','seeked','ended','durationchange','timeupdate','play','pause','ratechange','resize','volumechange']";

#[test]
fn audio_element_plays_blob_wav_with_spec_events() {
    null_audio();
    let (mut env, mut host) = setup("<html><body></body></html>");
    run(&mut env, &mut host, &format!(
        r#"{WAV_JS}
        const a = new Audio();
        window.log = [];
        for (const e of {EVENTS}) a.addEventListener(e, () => log.push(e));
        a.muted = true;
        a.src = URL.createObjectURL(new Blob([wav(0.6, 8000)], {{ type: 'audio/wav' }}));
        window.a = a;
        a.play().then(() => log.push('resolved'), (e) => log.push('rejected:' + e.name));
        "#
    ));
    let out = pump(&mut env, &mut host, 30.0, "a.ended", "[a.ended, a.paused, a.readyState, a.networkState, Math.abs(a.currentTime - a.duration) < 0.05, a.duration.toFixed(2), a.played.length].join(',') + '|' + log.filter(e => e !== 'timeupdate' && e !== 'progress').join(',')");
    let (state, events) = out.trim_matches('\'').split_once('|').unwrap();
    assert_eq!(state, "true,true,4,1,true,0.60,1", "{out}");
    let ev: Vec<&str> = events.split(',').collect();
    let pos = |e: &str| ev.iter().position(|x| *x == e).unwrap_or_else(|| panic!("missing {e} in {events}"));
    assert!(pos("volumechange") < pos("loadstart") || pos("loadstart") < pos("loadedmetadata"));
    assert!(pos("play") < pos("playing"));
    assert!(pos("loadstart") < pos("durationchange") && pos("durationchange") < pos("loadedmetadata"));
    assert!(pos("loadedmetadata") < pos("loadeddata") && pos("loadeddata") < pos("canplay"));
    assert!(pos("canplay") < pos("playing") && pos("playing") < pos("resolved"));
    assert!(pos("canplaythrough") > pos("canplay"));
    assert_eq!(*ev.last().unwrap(), "ended", "{events}");
    assert!(pos("pause") < pos("ended"));
    let errors = env.errors();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn autoplay_policy_seek_and_errors() {
    null_audio();
    let (mut env, mut host) = setup("<html><body><video id=v></video></body></html>");
    run(&mut env, &mut host, &format!(
        r#"{WAV_JS}
        window.log = [];
        const a = document.createElement('audio');
        a.src = URL.createObjectURL(new Blob([wav(2, 8000)], {{ type: 'audio/wav' }}));
        window.a = a;
        log.push('ns:' + a.networkState);
        a.play().then(() => log.push('resolved'), (e) => log.push('rejected:' + e.name));
        a.addEventListener('seeking', () => log.push('seeking'));
        a.addEventListener('seeked', () => log.push('seeked:' + a.currentTime.toFixed(1)));
        const bad = document.createElement('audio');
        bad.addEventListener('error', () => log.push('error:' + bad.error.code + ':' + bad.networkState));
        // (The mock environment's blob URLs are not unique: use data:.)
        bad.src = 'data:audio/wav;base64,' + btoa('not media at all'.repeat(40));
        window.ct = [a.canPlayType('audio/mpeg'), a.canPlayType('audio/ogg; codecs="opus"'), a.canPlayType('video/mp4; codecs="avc1.42E01E"'), a.canPlayType('audio/webm; codecs=vorbis'), a.canPlayType('application/x-foo'),
          MediaSource.isTypeSupported('audio/webm; codecs="opus"'), MediaSource.isTypeSupported('audio/mp4; codecs="mp4a.40.2"'), MediaSource.isTypeSupported('audio/ogg')].join(',');
        "#
    ));
    let out = pump(&mut env, &mut host, 30.0, "a.readyState === 4 && log.some(e => e.startsWith('error'))", "log.join(',') + '|' + [a.networkState, a.readyState, a.currentSrc.slice(0, 5)].join(':') + '|' + ct");
    assert!(out.contains("rejected:NotAllowedError"), "{out}");
    assert!(out.contains("error:4:3"), "{out}");
    assert!(out.ends_with("|probably,probably,,probably,,true,true,false'"), "{out}");
    run(&mut env, &mut host, "a.currentTime = 1.5; log.push('set:' + a.seeking + ':' + a.readyState);");
    let out = pump(&mut env, &mut host, 30.0, "log.some(e => e.startsWith('seeked'))", "log.join(',') + '|' + a.currentTime.toFixed(1) + '|' + a.seeking + ':' + a.readyState");
    assert!(out.contains("set:true:4,seeking,seeked:1.5"), "{out}");
    assert!(out.ends_with("|1.5|false:4'"), "{out}");
}

#[test]
fn media_source_append_end_of_stream_and_play() {
    null_audio();
    use rabbithole_media::demux::webm::build as wb;
    let toc = [(31u8 << 3) | 0b100];
    let blocks: Vec<(i16, &[u8])> = (0..50).map(|i| ((i * 20) as i16, &toc[..])).collect();
    let init = wb::audio_init("A_OPUS", b"OpusHead\x01\x02\x38\x01\x80\xbb\0\0\0\0\0", 48000.0, 2, None);
    let media = wb::cluster(0, &blocks);
    let (mut env, mut host) = setup("<html><body><audio id=a></audio></body></html>");
    let iv = crate::fetch::make_array_buffer(host.realm(), init);
    host.realm().define_global("INIT", iv);
    let mv = crate::fetch::make_array_buffer(host.realm(), media);
    host.realm().define_global("MEDIA", mv);
    run(&mut env, &mut host, r#"
        window.log = [];
        const a = document.getElementById('a');
        window.a = a;
        a.muted = true;
        const ms = new MediaSource();
        window.ms = ms;
        log.push(ms.readyState);
        ms.addEventListener('sourceopen', () => {
          log.push('sourceopen:' + ms.readyState + ':' + ms.duration);
          const sb = ms.addSourceBuffer('audio/webm; codecs="opus"');
          window.sb = sb;
          for (const e of ['updatestart', 'update', 'updateend']) sb.addEventListener(e, () => log.push(e));
          sb.appendBuffer(INIT);
          sb.addEventListener('updateend', () => {
            if (!window.second) { window.second = true; sb.appendBuffer(new Uint8Array(MEDIA)); }
            else { log.push('buffered:' + sb.buffered.length + ':' + sb.buffered.end(0).toFixed(2)); ms.endOfStream(); log.push(ms.readyState + ':' + ms.duration.toFixed(2)); a.play(); }
          });
        });
        a.addEventListener('ended', () => log.push('ended'));
        a.addEventListener('loadedmetadata', () => log.push('loadedmetadata:' + a.duration));
        a.src = URL.createObjectURL(ms);
    "#);
    let out = pump(&mut env, &mut host, 30.0, "log.includes('ended')", "log.join(',') + '|' + a.currentTime.toFixed(2) + '|' + ms.sourceBuffers.length + ',' + ms.activeSourceBuffers.length");
    assert!(out.starts_with("'closed,sourceopen:open:NaN,updatestart,update,updateend"), "{out}");
    assert!(out.contains("loadedmetadata:Infinity") || out.contains("loadedmetadata:1"), "{out}");
    assert!(out.contains("buffered:1:1.00"), "{out}");
    assert!(out.contains("ended:1.00"), "{out}");
    assert!(out.ends_with("ended|1.00|1,1'"), "{out}");
}

#[test]
fn web_audio_decode_play_and_offline_render() {
    null_audio();
    let (mut env, mut host) = setup("<html><body></body></html>");
    run(&mut env, &mut host, &format!(
        r#"{WAV_JS}
        window.log = [];
        const ctx = new AudioContext();
        log.push(ctx.state, ctx.sampleRate);
        ctx.resume();
        ctx.decodeAudioData(wav(0.3, 8000)).then((b) => {{
          log.push('decoded:' + b.numberOfChannels + ':' + b.sampleRate + ':' + b.length + ':' + (b.getChannelData(0).some((x) => Math.abs(x) > 0.1)));
          const src = ctx.createBufferSource();
          const gain = ctx.createGain();
          gain.gain.value = 0.5;
          src.buffer = b;
          src.connect(gain).connect(ctx.destination);
          src.onended = () => log.push('ended');
          src.start();
        }}, (e) => log.push('decode error ' + e));
        ctx.decodeAudioData(new ArrayBuffer(10)).catch((e) => log.push('bad:' + e.name));
        const off = new OfflineAudioContext(1, 100, 8000);
        const osc = off.createOscillator();
        osc.frequency.value = 1000;
        osc.connect(off.destination);
        osc.start();
        off.startRendering().then((b) => log.push('offline:' + b.length + ':' + b.getChannelData(0)[2].toFixed(3)));
        "#
    ));
    let out = pump(&mut env, &mut host, 30.0, "log.includes('ended')", "log.join(',')");
    assert!(out.starts_with("'suspended,48000"), "{out}");
    assert!(out.contains("decoded:1:48000:14400:true"), "{out}");
    assert!(out.contains("bad:EncodingError"), "{out}");
    assert!(out.contains("offline:100:1.000"), "{out}");
    assert!(out.contains("ended"), "{out}");
}
