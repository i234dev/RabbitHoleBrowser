//! `Intl.Segmenter` (ECMA-402 §18): Segments objects (`containing`,
//! `@@iterator`), segment iterators, and grapheme / word / sentence
//! boundaries after a simplified UAX #29.

use super::duration::{get_options_object, get_string_option};
use super::*;
use crate::regex::has_property;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct SegmenterOptions {
    locale: Locale,
    granularity: String,
}

/// %Segments% data: the string, granularity and boundaries (UTF-16).
#[derive(Debug, Clone)]
struct SegmentsData {
    input: JsStr,
    granularity: String,
    /// Segment start offsets plus the string length at the end.
    breaks: Rc<Vec<usize>>,
    /// Word-likeness of each segment (word granularity).
    word_like: Rc<Vec<bool>>,
}

/// %SegmentIterator% data.
#[derive(Debug, Clone)]
struct IteratorData {
    segments: SegmentsData,
    next: usize,
}

pub(super) fn ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if realm.new_target().is_undefined() {
        return realm.type_error("Constructor Intl.Segmenter requires 'new'");
    }
    let locale = lookup_locale(realm, arg(args, 0))?;
    let opts = get_options_object(realm, arg(args, 1))?;
    get_string_option(realm, opts, "localeMatcher", &["lookup", "best fit"], Some("best fit"))?;
    let granularity = get_string_option(realm, opts, "granularity", &["grapheme", "word", "sentence"], Some("grapheme"))?.unwrap();
    Ok(Value::Object(new_intl_object(realm, "Intl.Segmenter", SegmenterOptions { locale, granularity })?))
}

pub(super) fn resolved_options(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let o: SegmenterOptions = host(realm, &this, "Intl.Segmenter.prototype.resolvedOptions")?;
    let obj = realm.new_object();
    realm.define_property(obj, "locale", Value::str(&o.locale.tag), ALL);
    realm.define_property(obj, "granularity", Value::str(&o.granularity), ALL);
    Ok(Value::Object(obj))
}

// ------------------------------------------------------------ boundaries

/// Code points of `units` with their UTF-16 start offsets (lone surrogates
/// stand for themselves).
fn code_points(units: &[u16]) -> Vec<(usize, u32)> {
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = units[i] as u32;
        if (0xD800..0xDC00).contains(&u) && i + 1 < units.len() && (0xDC00..0xE000).contains(&(units[i + 1] as u32)) {
            let c = 0x10000 + ((u - 0xD800) << 10) + (units[i + 1] as u32 - 0xDC00);
            out.push((i, c));
            i += 2;
        } else {
            out.push((i, u));
            i += 1;
        }
    }
    out
}

fn is_extend(c: u32) -> bool {
    c == 0x200D || (0xFE00..=0xFE0F).contains(&c) || (0x1F3FB..=0x1F3FF).contains(&c) || (0xE0020..=0xE007F).contains(&c) || has_property(c, "M")
}

fn is_regional(c: u32) -> bool {
    (0x1F1E6..=0x1F1FF).contains(&c)
}

#[derive(PartialEq, Clone, Copy)]
enum Hangul {
    L,
    V,
    T,
    Lv,
    Lvt,
    None,
}

fn hangul(c: u32) -> Hangul {
    match c {
        0x1100..=0x115F | 0xA960..=0xA97C => Hangul::L,
        0x1160..=0x11A7 | 0xD7B0..=0xD7C6 => Hangul::V,
        0x11A8..=0x11FF | 0xD7CB..=0xD7FB => Hangul::T,
        0xAC00..=0xD7A3 => {
            if (c - 0xAC00) % 28 == 0 {
                Hangul::Lv
            } else {
                Hangul::Lvt
            }
        }
        _ => Hangul::None,
    }
}

/// Grapheme cluster boundary between code points `a` and `b` (`ri_run`:
/// regional indicators before `b` in the current run).
fn grapheme_break(a: u32, b: u32, ri_run: usize) -> bool {
    if a == 0x0D && b == 0x0A {
        return false;
    }
    if matches!(a, 0x0A | 0x0D) || matches!(b, 0x0A | 0x0D) {
        return true;
    }
    let (ha, hb) = (hangul(a), hangul(b));
    if ha == Hangul::L && matches!(hb, Hangul::L | Hangul::V | Hangul::Lv | Hangul::Lvt) {
        return false;
    }
    if matches!(ha, Hangul::Lv | Hangul::V) && matches!(hb, Hangul::V | Hangul::T) {
        return false;
    }
    if matches!(ha, Hangul::Lvt | Hangul::T) && hb == Hangul::T {
        return false;
    }
    if is_extend(b) && !(0xD800..0xE000).contains(&a) {
        return false;
    }
    if a == 0x200D && has_property(b, "Extended_Pictographic") {
        return false;
    }
    if is_regional(a) && is_regional(b) && ri_run % 2 == 1 {
        return false;
    }
    true
}

#[derive(PartialEq, Clone, Copy)]
enum WordClass {
    Letter,
    Ideo,
    Kana,
    Space,
    Other,
}

fn word_class(c: u32) -> WordClass {
    if matches!(c, 0x09 | 0x0B | 0x0C | 0x20 | 0x0A | 0x0D | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000) {
        return WordClass::Space;
    }
    if (0x30A0..=0x30FF).contains(&c) || (0x31F0..=0x31FF).contains(&c) || (0xFF66..=0xFF9F).contains(&c) {
        return WordClass::Kana;
    }
    if has_property(c, "Ideographic") || (0x3040..=0x309F).contains(&c) || (0x0E00..=0x0E7F).contains(&c) {
        return WordClass::Ideo;
    }
    if has_property(c, "L") || has_property(c, "N") || c == 0x5F {
        return WordClass::Letter;
    }
    WordClass::Other
}

/// Breaks (UTF-16 offsets) and word-likeness for `units`.
fn boundaries(units: &[u16], granularity: &str) -> (Vec<usize>, Vec<bool>) {
    let cps = code_points(units);
    let mut breaks = Vec::new();
    let mut word_like = Vec::new();
    if cps.is_empty() {
        breaks.push(0);
        return (breaks, word_like);
    }
    // Graphemes first: words and sentences never split a cluster.
    let mut clusters: Vec<(usize, u32)> = Vec::new();
    let mut ri_run = 0;
    for (k, &(off, c)) in cps.iter().enumerate() {
        if k == 0 {
            clusters.push((off, c));
        } else {
            let prev = cps[k - 1].1;
            if grapheme_break(prev, c, ri_run) {
                clusters.push((off, c));
            }
        }
        ri_run = if is_regional(c) { ri_run + 1 } else { 0 };
    }
    match granularity {
        "word" => {
            let classes: Vec<WordClass> = clusters.iter().map(|&(_, c)| word_class(c)).collect();
            let mut k = 0;
            while k < clusters.len() {
                breaks.push(clusters[k].0);
                let cl = classes[k];
                let mut j = k + 1;
                match cl {
                    WordClass::Letter => {
                        while j < clusters.len() {
                            if classes[j] == WordClass::Letter {
                                j += 1;
                            } else if j + 1 < clusters.len() && classes[j + 1] == WordClass::Letter && {
                                // WB6/7 (MidLetter, MidNumLet) between letters; WB11/12
                                // (MidNum, MidNumLet) between digits.
                                let (prev, mid, next) = (clusters[j - 1].1, clusters[j].1, clusters[j + 1].1);
                                let digit = |c: u32| has_property(c, "Nd");
                                match mid {
                                    0x2E | 0x27 | 0x2019 => true,
                                    0x3A => !digit(prev) && !digit(next),
                                    0x2C | 0x3B => digit(prev) && digit(next),
                                    _ => false,
                                }
                            } {
                                j += 2;
                            } else {
                                break;
                            }
                        }
                    }
                    WordClass::Kana => {
                        while j < clusters.len() && classes[j] == WordClass::Kana {
                            j += 1;
                        }
                    }
                    WordClass::Space => {
                        while j < clusters.len() && classes[j] == WordClass::Space && !matches!(clusters[j].1, 0x0A | 0x0D) && !matches!(clusters[j - 1].1, 0x0A | 0x0D) {
                            j += 1;
                        }
                    }
                    _ => {}
                }
                word_like.push(matches!(cl, WordClass::Letter | WordClass::Ideo | WordClass::Kana));
                k = j;
            }
        }
        "sentence" => {
            let mut k = 0;
            breaks.push(0);
            while k < clusters.len() {
                let c = clusters[k].1;
                k += 1;
                let terminal = matches!(c, 0x2E | 0x3F | 0x21 | 0x3002 | 0xFF01 | 0xFF1F);
                let paragraph = matches!(c, 0x0A | 0x0D | 0x2029 | 0x2028 | 0x85);
                if terminal {
                    while k < clusters.len() && matches!(clusters[k].1, 0x2E | 0x3F | 0x21 | 0x22 | 0x27 | 0x29 | 0x5D | 0x3002) {
                        k += 1;
                    }
                    let had_space = k < clusters.len() && word_class(clusters[k].1) == WordClass::Space;
                    while k < clusters.len() && word_class(clusters[k].1) == WordClass::Space {
                        k += 1;
                    }
                    // "1.23" or "e.g." without a following space do not end a sentence.
                    if k < clusters.len() && (had_space || c != 0x2E) && *breaks.last().unwrap() != clusters[k].0 {
                        breaks.push(clusters[k].0);
                    }
                } else if paragraph && k < clusters.len() {
                    breaks.push(clusters[k].0);
                }
            }
        }
        _ => {
            breaks.extend(clusters.iter().map(|&(o, _)| o));
        }
    }
    breaks.push(units.len());
    (breaks, word_like)
}

// --------------------------------------------------------------- objects

pub(super) fn segment(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let o: SegmenterOptions = host(realm, &this, "Intl.Segmenter.prototype.segment")?;
    let input = realm.to_string(arg(args, 0))?;
    let (breaks, word_like) = boundaries(input.as_units(), &o.granularity);
    let data = SegmentsData { input, granularity: o.granularity, breaks: Rc::new(breaks), word_like: Rc::new(word_like) };
    let proto = realm.intrinsics.others["%SegmentsPrototype%"];
    let obj = realm.new_object_with_proto(Some(proto));
    realm.set_host_data(obj, Box::new(data));
    Ok(Value::Object(obj))
}

/// CreateSegmentDataObject for segment `k`.
fn segment_object(realm: &mut Realm, d: &SegmentsData, k: usize) -> Value {
    let (start, end) = (d.breaks[k], d.breaks[k + 1]);
    let obj = realm.new_object();
    realm.define_property(obj, "segment", Value::String(d.input.slice(start, end)), ALL);
    realm.define_property(obj, "index", Value::Number(start as f64), ALL);
    realm.define_property(obj, "input", Value::String(d.input.clone()), ALL);
    if d.granularity == "word" {
        realm.define_property(obj, "isWordLike", Value::Bool(d.word_like.get(k).copied().unwrap_or(false)), ALL);
    }
    Value::Object(obj)
}

fn this_segments(realm: &mut Realm, this: &Value, what: &str) -> JsResult<SegmentsData> {
    host::<SegmentsData>(realm, this, what)
}

pub(super) fn containing(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let d = this_segments(realm, &this, "%Segments.prototype%.containing")?;
    let n = realm.to_integer_or_infinity(arg(args, 0))?;
    let len = d.input.as_units().len();
    if n < 0.0 || n >= len as f64 {
        return Ok(Value::Undefined);
    }
    let n = n as usize;
    let k = match d.breaks.binary_search(&n) {
        Ok(k) => k,
        Err(k) => k - 1,
    };
    Ok(segment_object(realm, &d, k))
}

pub(super) fn segments_iterator(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let d = this_segments(realm, &this, "%Segments.prototype%[@@iterator]")?;
    let proto = realm.intrinsics.others["%SegmentIteratorPrototype%"];
    let obj = realm.new_object_with_proto(Some(proto));
    realm.set_host_data(obj, Box::new(IteratorData { segments: d, next: 0 }));
    Ok(Value::Object(obj))
}

pub(super) fn iterator_next(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let it: IteratorData = host(realm, &this, "%SegmentIterator.prototype%.next")?;
    let V::Object(id) = this.v() else { unreachable!() };
    let count = it.segments.breaks.len() - 1;
    let result = realm.new_object();
    if it.next >= count {
        realm.define_property(result, "value", Value::Undefined, ALL);
        realm.define_property(result, "done", Value::Bool(true), ALL);
        return Ok(Value::Object(result));
    }
    let v = segment_object(realm, &it.segments, it.next);
    if let Some(d) = realm.host_data_mut::<IteratorData>(id) {
        d.next += 1;
    }
    realm.define_property(result, "value", v, ALL);
    realm.define_property(result, "done", Value::Bool(false), ALL);
    Ok(Value::Object(result))
}

/// Create %SegmentsPrototype% and %SegmentIteratorPrototype%.
pub(super) fn register_prototypes(realm: &mut Realm) {
    let op = realm.intrinsics.object_proto;
    let segs = realm.new_object_with_proto(Some(op));
    realm.define_method(segs, "containing", 1, containing);
    let f = realm.new_native_function("[Symbol.iterator]", 0, segments_iterator);
    realm.define_property(segs, PropertyKey::Symbol(crate::realm::SYM_ITERATOR), Value::Object(f), WRITABLE | CONFIGURABLE);
    realm.intrinsics.others.insert("%SegmentsPrototype%", segs);
    let iter_proto = realm.intrinsics.iterator_proto;
    let it = realm.new_object_with_proto(Some(iter_proto));
    realm.define_method(it, "next", 0, iterator_next);
    realm.define_property(it, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Segmenter String Iterator"), CONFIGURABLE);
    realm.intrinsics.others.insert("%SegmentIteratorPrototype%", it);
}
