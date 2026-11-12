//! Native side of `HTMLMediaElement`, `MediaSource` and `SourceBuffer`
//! (the element state machine lives in `js/media.js`): media players and
//! MediaSource states from `rabbithole-media`, addressed by numeric ids.

use crate::env::with_env;
use crate::host::*;
use rabbithole_js::{JsResult, ObjectId, Realm, Value};
use rabbithole_media::mse::{MseHandle, SourceBufferState};
use rabbithole_media::player::{Player, PlayerSource, Status};
use rabbithole_media::source::{ByteFetcher, FetchStream};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Default)]
pub struct MediaState {
    players: HashMap<u32, Player>,
    sources: HashMap<u32, MseHandle>,
    next: u32,
    /// `js/media.js` callback for element mutations: (element, what).
    hook: Option<Value>,
    /// Mutations to report: (element, "src" | "source" | "inserted" | "removed").
    pending: Vec<(NodeRef, &'static str)>,
    /// Web Audio voices.
    voices: HashMap<u32, rabbithole_media::AudioSink>,
    /// Last video frame serial painted per player.
    frame_serials: HashMap<u32, u64>,
}

fn is_media_tag(t: &str) -> bool {
    t == "audio" || t == "video"
}

/// An attribute changed (from `set_attribute`).
pub(crate) fn note_attribute(n: NodeRef, name: &str) {
    if name != "src" || !hooked() {
        return;
    }
    let media = with_doc(n.doc, |d| d.element(n.node).is_some_and(|e| is_media_tag(&e.tag)));
    if media {
        with_media(|m| m.pending.push((n, "src")));
        rabbithole_js::request_post_native();
    }
}

/// Children were inserted / removed (from `record_child_list`).
pub(crate) fn note_child_list(parent: NodeRef, added: &[NodeRef], removed: &[NodeRef]) {
    if !hooked() {
        return;
    }
    let mut found: Vec<(NodeRef, &'static str)> = Vec::new();
    with_doc(parent.doc, |d| {
        let parent_media = d.element(parent.node).is_some_and(|e| is_media_tag(&e.tag));
        for (list, what) in [(added, "inserted"), (removed, "removed")] {
            for a in list {
                if parent_media && what == "inserted" && d.element(a.node).is_some_and(|e| e.tag == "source") {
                    found.push((parent, "source"));
                }
                for x in d.descendants(a.node) {
                    if d.element(x).is_some_and(|e| is_media_tag(&e.tag)) {
                        found.push((NodeRef { doc: a.doc, node: x }, what));
                    }
                }
            }
        }
    });
    if !found.is_empty() {
        with_media(|m| m.pending.extend(found));
        rabbithole_js::request_post_native();
    }
}

fn hooked() -> bool {
    with_media(|m| m.hook.is_some())
}

/// Report recorded mutations to `js/media.js`, right after the DOM call
/// that made them (setting `src` runs the load algorithm synchronously).
pub(crate) fn run_pending(realm: &mut Realm) {
    let (hook, pending) = with_media(|m| (m.hook.clone(), std::mem::take(&mut m.pending)));
    let Some(hook) = hook else { return };
    for (n, what) in pending {
        let el = wrap(realm, n);
        call_logged(realm, &hook, Value::Undefined, &[el, Value::str(what)], "media element");
    }
}

fn media_set_mutation_hook(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let f = arg(args, 0);
    root_value(realm, &f);
    with_media(|m| m.hook = Some(f));
    Ok(Value::Undefined)
}

/// `mediaSetUi(el, paused, time, duration, muted, volume, bufferedEnd)`:
/// what the element's native controls show.
fn media_set_ui(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(n) = node_of(realm, &arg(args, 0)) else { return Ok(Value::Undefined) };
    let paused = realm.to_boolean(&arg(args, 1));
    let time = num(realm, args, 2)?;
    let duration = num(realm, args, 3)?;
    let muted = realm.to_boolean(&arg(args, 4));
    let volume = num(realm, args, 5)?;
    let buffered_end = num(realm, args, 6)?;
    let ui = rabbithole_dom::MediaUi { paused, time, duration, muted, volume, buffered_end };
    let changed = with_doc(n.doc, |d| d.media_ui.insert(n.node, ui) != Some(ui));
    if changed && n.doc == 0 {
        mark_dirty();
    }
    Ok(Value::Undefined)
}

/// `mediaFrame(id, el)`: paint the player's current video picture into
/// the element when it changed. Returns whether it did.
fn media_frame(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = id_arg(realm, args, 0)?;
    let Some(n) = node_of(realm, &arg(args, 1)) else { return Ok(Value::Bool(false)) };
    let f = with_media(|m| {
        let last = m.frame_serials.get(&id).copied().unwrap_or(0);
        let f = m.players.get(&id).and_then(|p| p.video_frame(last));
        if let Some((serial, _)) = &f {
            m.frame_serials.insert(id, *serial);
        }
        f
    });
    let Some((_, frame)) = f else { return Ok(Value::Bool(false)) };
    if n.doc == 0 {
        let rgba = frame.rgba.as_ref().clone();
        with_env(|env| env.set_canvas_image(n.node, frame.width, frame.height, Some(rgba)));
    }
    Ok(Value::Bool(true))
}

/// `mediaVideoStats(id)` -> [shown, dropped]
fn media_video_stats(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = id_arg(realm, args, 0)?;
    let (shown, dropped) = with_media(|m| m.players.get(&id).map(|p| p.video_stats())).unwrap_or((0, 0));
    Ok(array(realm, vec![Value::Number(shown as f64), Value::Number(dropped as f64)]))
}

/// `mediaTime(id)`: the playback position now.
fn media_time(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = id_arg(realm, args, 0)?;
    Ok(with_media(|m| m.players.get(&id).map(|p| Value::Number(p.time()))).unwrap_or(Value::Null))
}

fn with_media<R>(f: impl FnOnce(&mut MediaState) -> R) -> R {
    let st = state();
    let mut st = st.borrow_mut();
    f(&mut st.media)
}

/// Range requests through the page's fetcher (cookies, headers, referer).
struct NetFetcher {
    fetcher: rabbithole_net::Fetcher,
    referer: rabbithole_net::Url,
}

impl ByteFetcher for NetFetcher {
    fn open(&self, url: &str, start: u64) -> Result<FetchStream, String> {
        let u = rabbithole_net::Url::parse_absolute(url).ok_or("invalid URL")?;
        let mut req = rabbithole_net::Request::get(u).with_referer(self.referer.clone());
        if req.url.is_http() {
            req = req.with_header("Range", &format!("bytes={start}-")).with_header("Accept", "*/*");
        }
        let resp = self.fetcher.request_stream(&req).map_err(|e| e.to_string())?;
        if resp.status >= 400 {
            return Err(format!("HTTP {}", resp.status));
        }
        let (offset, total) = if resp.status == 206 {
            // Content-Range: bytes s-e/total
            let cr = resp.header("content-range").unwrap_or("");
            let spec = cr.trim().trim_start_matches("bytes").trim();
            let (range, total) = spec.split_once('/').unwrap_or((spec, "*"));
            let s = range.split('-').next().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(start);
            (s, total.trim().parse::<u64>().ok())
        } else {
            (0, resp.content_length)
        };
        Ok(FetchStream { total, offset, content_type: resp.content_type.clone(), reader: resp.reader })
    }
}

fn num(realm: &mut Realm, args: &[Value], i: usize) -> JsResult<f64> {
    realm.to_number(arg(args, i))
}

fn id_arg(realm: &mut Realm, args: &[Value], i: usize) -> JsResult<u32> {
    Ok(num(realm, args, i)? as u32)
}

/// `mediaCreate(url, msId)` → player id, or a string error.
fn media_create(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let url = arg_str(realm, args, 0)?;
    let ms = arg(args, 1);
    let source = if !matches!(ms, Value::Undefined | Value::Null) {
        let ms = realm.to_number(ms)? as u32;
        match with_media(|m| m.sources.get(&ms).cloned()) {
            Some(h) => PlayerSource::Mse(h),
            None => return Ok(Value::str("MediaSource not found")),
        }
    } else if url.starts_with("blob:") {
        match crate::fetch::blob_url_response(&url) {
            Some(r) => PlayerSource::Bytes { data: r.body, mime: r.content_type },
            None => return Ok(Value::str("blob URL not found")),
        }
    } else {
        let fetcher = with_env(|env| env.media_fetcher()).unwrap_or_else(rabbithole_net::Fetcher::isolated);
        let referer = with_env(|env| env.document_url());
        PlayerSource::Url { url, fetcher: Arc::new(NetFetcher { fetcher, referer }) as Arc<dyn ByteFetcher> }
    };
    let player = Player::new(source);
    let id = with_media(|m| {
        m.next += 1;
        m.players.insert(m.next, player);
        m.next
    });
    Ok(Value::Number(id as f64))
}

fn media_destroy(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = id_arg(realm, args, 0)?;
    let p = with_media(|m| {
        m.frame_serials.remove(&id);
        m.players.remove(&id)
    });
    drop(p);
    Ok(Value::Undefined)
}

fn ranges(realm: &mut Realm, r: &[(f64, f64)]) -> Value {
    let items: Vec<Value> = r.iter().map(|&(s, e)| array(realm, vec![Value::Number(s), Value::Number(e)])).collect();
    array(realm, items)
}

fn status_object(realm: &mut Realm, st: &Status) -> Value {
    let buffered = ranges(realm, &st.buffered);
    let seekable = ranges(realm, &st.seekable);
    let error = match &st.error {
        Some((code, msg)) => array(realm, vec![Value::Number(*code as f64), Value::str(msg)]),
        None => Value::Null,
    };
    object_from(
        realm,
        vec![
            ("time", Value::Number(st.time)),
            ("duration", Value::Number(st.duration)),
            ("ready", Value::Number(st.ready as f64)),
            ("hasAudio", Value::Bool(st.has_audio)),
            ("hasVideo", Value::Bool(st.has_video)),
            ("width", Value::Number(st.video_width as f64)),
            ("height", Value::Number(st.video_height as f64)),
            ("audioCodec", Value::str(&st.audio_codec)),
            ("videoCodec", Value::str(&st.video_codec)),
            ("buffered", buffered),
            ("seekable", seekable),
            ("seeking", Value::Bool(st.seeking)),
            ("ended", Value::Bool(st.ended)),
            ("idle", Value::Bool(st.idle)),
            ("error", error),
            ("seekSerial", Value::Number(st.seek_serial as f64)),
            ("decoded", Value::Number(st.decoded_frames as f64)),
        ],
    )
}

fn media_status(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = id_arg(realm, args, 0)?;
    let Some(st) = with_media(|m| m.players.get(&id).map(|p| p.status())) else { return Ok(Value::Null) };
    Ok(status_object(realm, &st))
}

fn with_player(realm: &mut Realm, args: &[Value], f: impl FnOnce(&Player, &[f64])) -> JsResult<Value> {
    let id = id_arg(realm, args, 0)?;
    let mut nums = Vec::new();
    for i in 1..args.len() {
        nums.push(realm.to_number(args[i].clone())?);
    }
    with_media(|m| {
        if let Some(p) = m.players.get(&id) {
            f(p, &nums);
        }
    });
    Ok(Value::Undefined)
}

fn media_play(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    with_player(realm, args, |p, _| p.play())
}
fn media_pause(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    with_player(realm, args, |p, _| p.pause())
}
fn media_seek(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    with_player(realm, args, |p, n| p.seek(n.first().copied().unwrap_or(0.0)))
}
/// `mediaVolume(id, volume, muted)`
fn media_volume(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    with_player(realm, args, |p, n| {
        p.set_volume(n.first().copied().unwrap_or(1.0));
        p.set_muted(n.get(1).copied().unwrap_or(0.0) != 0.0);
    })
}
fn media_rate(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    with_player(realm, args, |p, n| p.set_rate(n.first().copied().unwrap_or(1.0)))
}

/// `mediaCanPlayType(type)` → "", "maybe" or "probably".
fn media_can_play_type(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let t = arg_str(realm, args, 0)?;
    Ok(Value::str(can_play_type(&t)))
}

pub(crate) fn can_play_type(t: &str) -> &'static str {
    use rabbithole_media::demux::codec_from_rfc6381;
    let (base, codecs) = rabbithole_media::mse::split_mime(t);
    // Containers we demux and the audio codecs each may hold.
    let audio: &[&str] = match base.as_str() {
        "audio/mpeg" | "audio/mp3" | "audio/mpeg3" | "audio/x-mpeg" => &["mp3"],
        "audio/wav" | "audio/wave" | "audio/x-wav" | "audio/vnd.wave" => &["pcm", "1"],
        "audio/ogg" | "application/ogg" | "video/ogg" => &["vorbis", "opus", "flac"],
        "audio/opus" => &["opus"],
        "audio/flac" | "audio/x-flac" => &["flac"],
        "audio/webm" | "video/webm" | "audio/x-matroska" | "video/x-matroska" => &["vorbis", "opus"],
        "audio/mp4" | "video/mp4" | "audio/x-m4a" | "audio/m4a" | "audio/aac" | "audio/x-aac" | "video/quicktime" => &["aac", "mp3", "alac", "flac", "opus"],
        "audio/aiff" | "audio/x-aiff" => &["pcm"],
        _ => return "",
    };
    if codecs.is_empty() {
        // Types that name their codec (as Chrome answers).
        return match base.as_str() {
            "audio/mpeg" | "audio/mp3" | "audio/flac" | "audio/x-flac" | "audio/aac" | "audio/x-aac" | "audio/opus" => "probably",
            _ => "maybe",
        };
    }
    let video_ok = base.starts_with("video/");
    for c in &codecs {
        let name = codec_from_rfc6381(c);
        let name = if name.is_empty() && c == "1" { "pcm" } else { name };
        // Video tracks are demuxed (audio plays, the picture shows the
        // poster) but not decoded: answer only for audio codecs + AV1.
        if !(audio.contains(&name) || (video_ok && name == "av1")) {
            return "";
        }
    }
    "probably"
}

/// `msIsTypeSupported(type)`.
fn ms_is_type_supported(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let t = arg_str(realm, args, 0)?;
    Ok(Value::Bool(rabbithole_media::mse::is_type_supported(&t)))
}

fn ms_create(_realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = with_media(|m| {
        m.next += 1;
        m.sources.insert(m.next, MseHandle::new());
        m.next
    });
    Ok(Value::Number(id as f64))
}

fn ms_destroy(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = id_arg(realm, args, 0)?;
    with_media(|m| m.sources.remove(&id));
    Ok(Value::Undefined)
}

fn ms_handle(realm: &mut Realm, args: &[Value]) -> JsResult<Option<MseHandle>> {
    let id = id_arg(realm, args, 0)?;
    Ok(with_media(|m| m.sources.get(&id).cloned()))
}

/// `msAddSourceBuffer(ms, type)` → index or null (unsupported).
fn ms_add_source_buffer(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(h) = ms_handle(realm, args)? else { return Ok(Value::Null) };
    let mime = arg_str(realm, args, 1)?;
    let Some(sb) = SourceBufferState::new(&mime) else { return Ok(Value::Null) };
    let mut m = h.lock();
    m.buffers.push(sb);
    m.version += 1;
    Ok(Value::Number((m.buffers.len() - 1) as f64))
}

fn sb_op<R>(realm: &mut Realm, args: &[Value], f: impl FnOnce(&mut SourceBufferState, &mut rabbithole_media::mse::MseInner) -> R) -> JsResult<Option<R>> {
    let Some(h) = ms_handle(realm, args)? else { return Ok(None) };
    let idx = id_arg(realm, args, 1)? as usize;
    let mut m = h.lock();
    if idx >= m.buffers.len() {
        return Ok(None);
    }
    // Take the buffer out to borrow both.
    let mut sb = std::mem::replace(&mut m.buffers[idx], SourceBufferState::new("audio/webm").expect("webm parser"));
    let r = f(&mut sb, &mut m);
    m.buffers[idx] = sb;
    m.version += 1;
    Ok(Some(r))
}

fn ms_remove_source_buffer(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    sb_op(realm, args, |sb, _| {
        sb.removed = true;
        sb.remove(f64::NEG_INFINITY, f64::INFINITY);
    })?;
    Ok(Value::Undefined)
}

/// `sbAppend(ms, idx, bytes)` → { inits: [{ duration, tracks: [...] }],
/// timestampOffset } or { error } / { quota: true }.
fn sb_append(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (bytes, _) = crate::fetch::body_bytes_from_value(realm, &arg(args, 2))?;
    let r = sb_op(realm, args, |sb, m| {
        let pos = m.position;
        if !sb.evict(bytes.len(), pos) {
            return Err(None);
        }
        sb.append(&bytes).map_err(Some)
    })?;
    match r {
        None => Ok(object_from(realm, vec![("error", Value::str("no such SourceBuffer"))])),
        Some(Err(None)) => Ok(object_from(realm, vec![("quota", Value::Bool(true))])),
        Some(Err(Some(e))) => Ok(object_from(realm, vec![("error", Value::str(&e))])),
        Some(Ok(res)) => {
            let mut inits = Vec::new();
            for (tracks, duration) in &res.inits {
                let mut tv = Vec::new();
                for t in tracks {
                    let kind = match t.kind {
                        rabbithole_media::demux::TrackKind::Audio => "audio",
                        rabbithole_media::demux::TrackKind::Video => "video",
                        _ => "other",
                    };
                    let o = object_from(
                        realm,
                        vec![
                            ("id", Value::Number(t.id as f64)),
                            ("kind", Value::str(kind)),
                            ("codec", Value::str(&t.codec)),
                            ("width", Value::Number(t.width as f64)),
                            ("height", Value::Number(t.height as f64)),
                            ("channels", Value::Number(t.channels as f64)),
                            ("sampleRate", Value::Number(t.sample_rate as f64)),
                            ("encrypted", Value::Bool(t.encrypted)),
                        ],
                    );
                    tv.push(o);
                }
                let tracks = array(realm, tv);
                inits.push(object_from(realm, vec![("tracks", tracks), ("duration", duration.map(Value::Number).unwrap_or(Value::Null))]));
            }
            let inits = array(realm, inits);
            Ok(object_from(realm, vec![("inits", inits), ("timestampOffset", Value::Number(res.timestamp_offset)), ("frames", Value::Number(res.frames as f64))]))
        }
    }
}

fn sb_remove(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = num(realm, args, 2)?;
    let e = num(realm, args, 3)?;
    sb_op(realm, args, |sb, _| sb.remove(s, e))?;
    Ok(Value::Undefined)
}

fn sb_abort(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    sb_op(realm, args, |sb, _| sb.reset_parser())?;
    Ok(Value::Undefined)
}

fn sb_buffered(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let r = sb_op(realm, args, |sb, _| sb.buffered())?.unwrap_or_default();
    Ok(ranges(realm, &r))
}

/// `sbSet(ms, idx, what, a, b)`: timestampOffset / mode / appendWindow.
fn sb_set(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let what = arg_str(realm, args, 2)?;
    let a = num(realm, args, 3)?;
    let b = realm.to_number(arg(args, 4)).unwrap_or(f64::NAN);
    sb_op(realm, args, |sb, _| match what.as_str() {
        "timestampOffset" => sb.set_timestamp_offset(a),
        "mode" => sb.set_mode(a != 0.0),
        "appendWindow" => sb.append_window = (a, if b.is_nan() { f64::INFINITY } else { b }),
        _ => {}
    })?;
    Ok(Value::Undefined)
}

fn sb_change_type(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let mime = arg_str(realm, args, 2)?;
    Ok(Value::Bool(sb_op(realm, args, |sb, _| sb.change_type(&mime))?.unwrap_or(false)))
}

fn sb_parsing(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(sb_op(realm, args, |sb, _| sb.parsing_media_segment())?.unwrap_or(false)))
}

/// `msSet(ms, what, value)`: duration / ended.
fn ms_set(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(h) = ms_handle(realm, args)? else { return Ok(Value::Undefined) };
    let what = arg_str(realm, args, 1)?;
    let v = num(realm, args, 2)?;
    let mut m = h.lock();
    match what.as_str() {
        "duration" => m.duration = v,
        "ended" => m.ended = v != 0.0,
        _ => {}
    }
    m.version += 1;
    Ok(Value::Undefined)
}

/// `msInfo(ms)` → { buffered, highest } (buffered ranges and the highest
/// buffered presentation end across SourceBuffers).
fn ms_info(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(h) = ms_handle(realm, args)? else { return Ok(Value::Null) };
    let (b, highest) = {
        let m = h.lock();
        let highest = m.buffers.iter().filter(|b| !b.removed).flat_map(|b| b.buffered().last().map(|r| r.1)).fold(0.0f64, f64::max);
        (m.buffered(), highest)
    };
    let br = ranges(realm, &b);
    Ok(object_from(realm, vec![("buffered", br), ("highest", Value::Number(highest))]))
}

/// `mediaAutoplayEnv()`: `RH_AUTOPLAY=1` allows unmuted autoplay without
/// user activation (headless testing).
fn media_autoplay_env(_realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(std::env::var("RH_AUTOPLAY").is_ok_and(|v| v == "1")))
}

/// `mediaDebug(msg)`: log when `RH_MEDIA_DEBUG` is set.
fn media_debug(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if std::env::var("RH_MEDIA_DEBUG").is_ok() {
        let s = arg_str(realm, args, 0)?;
        eprintln!("[media] {s}");
    }
    Ok(Value::Undefined)
}

/// `audioStats()` → process-wide output statistics.
fn audio_stats(realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let s = rabbithole_media::audio_stats();
    Ok(object_from(
        realm,
        vec![
            ("backend", Value::str(s.backend)),
            ("rate", Value::Number(s.rate as f64)),
            ("deviceError", s.device_error.as_deref().map(Value::str).unwrap_or(Value::Null)),
            ("frames", Value::Number(s.frames as f64)),
            ("nonsilent", Value::Number(s.nonsilent_samples as f64)),
            ("peak", Value::Number(s.peak as f64)),
            ("checksum", Value::str(&format!("{:016x}", s.checksum))),
        ],
    ))
}

// ---------------------------------------------------------------- Web Audio

fn f32s(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn wa_voice_create(_realm: &mut Realm, _this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = with_media(|m| {
        m.next += 1;
        let sink = rabbithole_media::AudioSink::new();
        sink.flush(0.0);
        m.voices.insert(m.next, sink);
        m.next
    });
    Ok(Value::Number(id as f64))
}

/// `waVoicePush(id, Float32Array interleaved, channels, rate, pts, eos)`
fn wa_voice_push(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = id_arg(realm, args, 0)?;
    let (bytes, _) = crate::fetch::body_bytes_from_value(realm, &arg(args, 1))?;
    let channels = num(realm, args, 2)? as usize;
    let rate = num(realm, args, 3)? as u32;
    let pts = num(realm, args, 4)?;
    let eos = realm.to_boolean(&arg(args, 5));
    let data = f32s(&bytes);
    with_media(|m| {
        if let Some(v) = m.voices.get(&id) {
            v.push(pts, rate, channels, &data);
            v.set_eos(eos);
        }
    });
    Ok(Value::Undefined)
}

fn wa_voice_set(realm: &mut Realm, args: &[Value], f: impl FnOnce(&rabbithole_media::AudioSink, f64)) -> JsResult<Value> {
    let id = id_arg(realm, args, 0)?;
    let v = realm.to_number(arg(args, 1)).unwrap_or(0.0);
    with_media(|m| {
        if let Some(s) = m.voices.get(&id) {
            f(s, v);
        }
    });
    Ok(Value::Undefined)
}

fn wa_voice_play(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    wa_voice_set(realm, args, |s, v| s.set_playing(v != 0.0))
}
fn wa_voice_gain(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    wa_voice_set(realm, args, |s, v| s.set_volume(v as f32))
}
fn wa_voice_rate(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    wa_voice_set(realm, args, |s, v| s.set_speed(if v > 0.0 { v } else { 1.0 }))
}
fn wa_voice_destroy(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = id_arg(realm, args, 0)?;
    let v = with_media(|m| m.voices.remove(&id));
    drop(v);
    Ok(Value::Undefined)
}
/// `waVoiceState(id)` → { clock, buffered, drained }
fn wa_voice_state(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let id = id_arg(realm, args, 0)?;
    let Some((clock, buffered, drained)) = with_media(|m| m.voices.get(&id).map(|s| (s.clock(), s.buffered_secs(), s.drained()))) else { return Ok(Value::Null) };
    Ok(object_from(realm, vec![("clock", Value::Number(clock)), ("buffered", Value::Number(buffered)), ("drained", Value::Bool(drained))]))
}

/// `waDecode(arrayBuffer, rate)` → { rate, channels: [ArrayBuffer of f32] }
/// or an error string.
fn wa_decode(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let (bytes, _) = crate::fetch::body_bytes_from_value(realm, &arg(args, 0))?;
    let rate = num(realm, args, 1)? as u32;
    let t = std::time::Instant::now();
    let r = rabbithole_media::oneshot::decode_all(bytes, rate);
    rabbithole_js::exclude_from_watchdog(t.elapsed());
    match r {
        Err(e) => Ok(Value::str(&e)),
        Ok(d) => {
            let mut chans = Vec::new();
            for c in d.channels {
                let mut b = Vec::with_capacity(c.len() * 4);
                for s in c {
                    b.extend_from_slice(&s.to_le_bytes());
                }
                chans.push(crate::fetch::make_array_buffer(realm, b));
            }
            let chans = array(realm, chans);
            Ok(object_from(realm, vec![("rate", Value::Number(d.rate as f64)), ("channels", chans)]))
        }
    }
}

pub(crate) fn install(realm: &mut Realm, o: ObjectId) {
    realm.define_method(o, "waVoiceCreate", 0, wa_voice_create);
    realm.define_method(o, "waVoicePush", 6, wa_voice_push);
    realm.define_method(o, "waVoicePlay", 2, wa_voice_play);
    realm.define_method(o, "waVoiceGain", 2, wa_voice_gain);
    realm.define_method(o, "waVoiceRate", 2, wa_voice_rate);
    realm.define_method(o, "waVoiceDestroy", 1, wa_voice_destroy);
    realm.define_method(o, "waVoiceState", 1, wa_voice_state);
    realm.define_method(o, "waDecode", 2, wa_decode);
    realm.define_method(o, "mediaCreate", 2, media_create);
    realm.define_method(o, "mediaDestroy", 1, media_destroy);
    realm.define_method(o, "mediaStatus", 1, media_status);
    realm.define_method(o, "mediaTime", 1, media_time);
    realm.define_method(o, "mediaFrame", 2, media_frame);
    realm.define_method(o, "mediaVideoStats", 1, media_video_stats);
    realm.define_method(o, "mediaSetUi", 7, media_set_ui);
    realm.define_method(o, "mediaSetMutationHook", 1, media_set_mutation_hook);
    realm.define_method(o, "mediaPlay", 1, media_play);
    realm.define_method(o, "mediaPause", 1, media_pause);
    realm.define_method(o, "mediaSeek", 2, media_seek);
    realm.define_method(o, "mediaVolume", 3, media_volume);
    realm.define_method(o, "mediaRate", 2, media_rate);
    realm.define_method(o, "mediaCanPlayType", 1, media_can_play_type);
    realm.define_method(o, "mediaAutoplayEnv", 0, media_autoplay_env);
    realm.define_method(o, "mediaDebug", 1, media_debug);
    realm.define_method(o, "audioStats", 0, audio_stats);
    realm.define_method(o, "msIsTypeSupported", 1, ms_is_type_supported);
    realm.define_method(o, "msCreate", 0, ms_create);
    realm.define_method(o, "msDestroy", 1, ms_destroy);
    realm.define_method(o, "msAddSourceBuffer", 2, ms_add_source_buffer);
    realm.define_method(o, "msRemoveSourceBuffer", 2, ms_remove_source_buffer);
    realm.define_method(o, "msSet", 3, ms_set);
    realm.define_method(o, "msInfo", 1, ms_info);
    realm.define_method(o, "sbAppend", 3, sb_append);
    realm.define_method(o, "sbRemove", 4, sb_remove);
    realm.define_method(o, "sbAbort", 2, sb_abort);
    realm.define_method(o, "sbBuffered", 2, sb_buffered);
    realm.define_method(o, "sbSet", 5, sb_set);
    realm.define_method(o, "sbChangeType", 3, sb_change_type);
    realm.define_method(o, "sbParsing", 2, sb_parsing);
}

impl HostState {
    /// Live media players (the event loop polls them).
    pub fn media_active(&self) -> bool {
        !self.media.players.is_empty() || !self.media.voices.is_empty()
    }
}
