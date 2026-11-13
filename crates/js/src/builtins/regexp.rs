//! RegExp objects: construction, exec/test and the Symbol.match/replace/
//! search/split/matchAll protocol used by String.prototype.

use super::*;
use crate::realm::{SYM_MATCH, SYM_MATCH_ALL, SYM_REPLACE, SYM_SEARCH, SYM_SPECIES, SYM_SPLIT, SYM_TO_STRING_TAG};
use crate::regex::Regex;
use crate::value::V;
use std::rc::Rc;

pub fn register(realm: &mut Realm) {
    let rp = realm.new_object();
    realm.intrinsics.others.insert("RegExpPrototype", rp);
    let ctor = define_ctor(realm, "RegExp", 2, regexp_ctor, rp);
    realm.intrinsics.others.insert("RegExp", ctor);
    realm.define_accessor(ctor, PropertyKey::Symbol(SYM_SPECIES), Some(species), None, CONFIGURABLE);
    realm.define_method(ctor, "escape", 1, regexp_escape);
    realm.define_method(rp, "exec", 1, exec);
    realm.define_method(rp, "test", 1, test);
    realm.define_method(rp, "toString", 0, to_string);
    realm.define_method(rp, "compile", 2, compile);
    realm.define_accessor(rp, "source", Some(source), None, CONFIGURABLE);
    realm.define_accessor(rp, "flags", Some(flags), None, CONFIGURABLE);
    realm.define_accessor(rp, "global", Some(flag_global), None, CONFIGURABLE);
    realm.define_accessor(rp, "ignoreCase", Some(flag_ignore_case), None, CONFIGURABLE);
    realm.define_accessor(rp, "multiline", Some(flag_multiline), None, CONFIGURABLE);
    realm.define_accessor(rp, "dotAll", Some(flag_dot_all), None, CONFIGURABLE);
    realm.define_accessor(rp, "unicode", Some(flag_unicode), None, CONFIGURABLE);
    realm.define_accessor(rp, "unicodeSets", Some(flag_unicode_sets), None, CONFIGURABLE);
    realm.define_accessor(rp, "sticky", Some(flag_sticky), None, CONFIGURABLE);
    realm.define_accessor(rp, "hasIndices", Some(flag_has_indices), None, CONFIGURABLE);
    realm.define_symbol_method(rp, SYM_MATCH, "[Symbol.match]", 1, symbol_match);
    realm.define_symbol_method(rp, SYM_MATCH_ALL, "[Symbol.matchAll]", 1, symbol_match_all);
    realm.define_symbol_method(rp, SYM_REPLACE, "[Symbol.replace]", 2, symbol_replace);
    realm.define_symbol_method(rp, SYM_SEARCH, "[Symbol.search]", 1, symbol_search);
    realm.define_symbol_method(rp, SYM_SPLIT, "[Symbol.split]", 2, symbol_split);
    let ip = realm.intrinsics.iterator_proto;
    let rsip = realm.new_object_with_proto(Some(ip));
    realm.define_method(rsip, "next", 0, regexp_string_iterator_next);
    realm.define_property(rsip, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("RegExp String Iterator"), CONFIGURABLE);
    realm.intrinsics.others.insert("RegExpStringIteratorPrototype", rsip);
    let p = &realm.obj(rp).props;
    let slot_of = |name: &str| p.find_slot(&PropertyKey::str(name));
    let exec = slot_of("exec").and_then(|s| Some((s, p.slot(s)?.value.as_object()?)));
    let mut getters = Vec::new();
    for name in ["flags", "hasIndices", "global", "ignoreCase", "multiline", "dotAll", "unicode", "unicodeSets", "sticky"] {
        if let Some(g) = slot_of(name).and_then(|s| Some((s, p.slot(s)?.getter()?.as_object()?))) {
            getters.push(g);
        }
    }
    if let Some((exec_slot, exec)) = exec {
        if getters.len() == 9 {
            realm.regexp_pristine = Some(Box::new(Pristine { shape: p.shape_id(), exec_slot, exec, getters }));
        }
    }
}

/// `RegExp.prototype` as built: its shape, `exec`, and the getters of
/// `flags` and of each flag (slot, function).
pub struct Pristine {
    shape: u32,
    exec_slot: u32,
    exec: ObjectId,
    getters: Vec<(u32, ObjectId)>,
}

/// `v` is a RegExp object whose observable protocol is the built-in one:
/// only its own `lastIndex`, and `RegExp.prototype` with its original keys,
/// `exec`, `flags` and flag getters. Its flags and matches can then be used
/// directly.
fn pristine(realm: &Realm, v: &Value) -> Option<ObjectId> {
    let pr = realm.regexp_pristine.as_deref()?;
    let id = v.as_object()?;
    let o = realm.obj(id);
    if !matches!(o.kind, ObjectKind::RegExp(_)) || o.props.len() != 1 {
        return None;
    }
    let p = &realm.obj(o.proto?).props;
    if p.shape_id() != pr.shape || p.slot(pr.exec_slot)?.value.as_object() != Some(pr.exec) {
        return None;
    }
    for &(slot, g) in &pr.getters {
        if p.slot(slot)?.getter().and_then(|x| x.as_object()) != Some(g) {
            return None;
        }
    }
    Some(id)
}

/// Like [`pristine`], and its `lastIndex` is a writable number (reading and
/// writing it has no side effects).
fn pristine_plain(realm: &Realm, v: &Value) -> Option<ObjectId> {
    let id = pristine(realm, v)?;
    let o = realm.obj(id);
    let slot = o.props.find_slot(&PropertyKey::str("lastIndex"))?;
    let prop = o.props.slot(slot)?;
    if prop.getter().is_some() || prop.flags & WRITABLE == 0 || !matches!(prop.value.v(), V::Number(_)) {
        return None;
    }
    Some(id)
}

fn regex_of(realm: &Realm, id: ObjectId) -> Rc<Regex> {
    match &realm.obj(id).kind {
        ObjectKind::RegExp(d) => d.regex.clone(),
        _ => unreachable!(),
    }
}

/// `RegExp.prototype[Symbol.replace]` for a pristine RegExp: the matches
/// come straight from the regex engine (no result arrays / property reads).
fn fast_replace(realm: &mut Realm, rx: ObjectId, s: &JsStr, rep: &Value, functional: bool, rep_str: &JsStr) -> JsResult<Option<Value>> {
    let regex = regex_of(realm, rx);
    let (global, sticky, unicode) = (regex.flags.global, regex.flags.sticky, regex.flags.either_unicode());
    if sticky {
        return Ok(None);
    }
    let rxv = Value::Object(rx);
    if global {
        set_last_index(realm, &rxv, 0)?;
    }
    // A replacement string without `$` patterns is inserted as is.
    if !functional && !rep_str.as_units().contains(&(b'$' as u16)) {
        let units = s.as_units();
        let mut out: Vec<u16> = Vec::with_capacity(units.len());
        let (mut pos, mut next_pos) = (0usize, 0usize);
        let mut last = None;
        while pos <= units.len() {
            let Some(m) = regex.exec_utf16(units, pos) else { break };
            let (a, b) = (m.start(), m.end());
            out.extend_from_slice(&units[next_pos..a]);
            out.extend_from_slice(rep_str.as_units());
            next_pos = b;
            last = Some(m);
            if !global {
                break;
            }
            pos = if a == b { advance_index(s, b, unicode) } else { b };
        }
        if global {
            set_last_index(realm, &rxv, 0)?;
        }
        let Some(m) = last else { return Ok(Some(Value::String(s.clone()))) };
        realm.regexp_last = Some((s.clone(), m.captures));
        out.extend_from_slice(&units[next_pos..]);
        return Ok(Some(Value::String(JsStr::from_units(out))));
    }
    // (start, end, captures) of every match.
    let mut found: Vec<(usize, usize, Vec<Option<(usize, usize)>>)> = Vec::new();
    let mut pos = 0usize;
    loop {
        if pos > s.len() {
            break;
        }
        let Some(m) = regex.exec_utf16(s.as_units(), pos) else { break };
        let (a, b) = (m.start(), m.end());
        let caps: Vec<Option<(usize, usize)>> = m.captures.iter().skip(1).cloned().collect();
        realm.regexp_last = Some((s.clone(), m.captures));
        found.push((a, b, caps));
        if !global {
            break;
        }
        pos = if a == b { advance_index(s, b, unicode) } else { b };
    }
    let names = regex.named_groups();
    let mut out: Vec<u16> = Vec::with_capacity(s.len());
    let mut next_pos = 0usize;
    for (a, b, caps) in found {
        let matched = s.slice(a, b);
        let captures: Vec<Value> = caps
            .iter()
            .map(|c| match c {
                Some((x, y)) => Value::String(s.slice(*x, *y)),
                None => Value::Undefined,
            })
            .collect();
        let named = if names.is_empty() {
            Value::Undefined
        } else {
            let g = realm.new_object_with_proto(None);
            for (name, idxs) in names {
                let v = idxs.iter().map(|i| captures[*i - 1].clone()).find(|v| !v.is_undefined()).unwrap_or(Value::Undefined);
                realm.define_property(g, name.as_str(), v, ALL);
            }
            Value::Object(g)
        };
        let replacement = if functional {
            let mut fargs = Vec::with_capacity(captures.len() + 4);
            fargs.push(Value::String(matched.clone()));
            fargs.extend(captures.iter().cloned());
            fargs.push(Value::Number(a as f64));
            fargs.push(Value::String(s.clone()));
            if !named.is_undefined() {
                fargs.push(named.clone());
            }
            let rv = realm.call(rep.clone(), Value::Undefined, &fargs)?;
            realm.to_string(rv)?
        } else {
            string::get_substitution(realm, &matched, s, a, &captures, &named, rep_str)?
        };
        if a >= next_pos {
            out.extend_from_slice(&s.as_units()[next_pos..a]);
            out.extend_from_slice(replacement.as_units());
            next_pos = b;
        }
    }
    if next_pos < s.len() {
        out.extend_from_slice(&s.as_units()[next_pos..]);
    }
    Ok(Some(Value::String(JsStr::from_units(out))))
}

fn species(_realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(this)
}

/// The compiled regex for (pattern, flags), from a per-thread cache: a
/// literal inside a function compiles once, not on every evaluation.
fn compiled(pattern: &[u16], flags: &str) -> Result<Rc<Regex>, crate::regex::RegexError> {
    type Cache = std::collections::HashMap<(Box<[u16]>, Box<str>), Rc<Regex>, crate::value::FxBuild>;
    thread_local! {
        static CACHE: std::cell::RefCell<Cache> = std::cell::RefCell::new(Cache::default());
    }
    let key: (Box<[u16]>, Box<str>) = (pattern.into(), flags.into());
    if let Some(r) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return Ok(r);
    }
    let r = Rc::new(Regex::new_utf16(pattern, flags)?);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= 2048 {
            c.clear();
        }
        c.insert(key, r.clone());
    });
    Ok(r)
}

fn syntax_error(realm: &mut Realm, pattern: &JsStr, flags: &str, e: &crate::regex::RegexError) -> Value {
    realm.new_error("SyntaxError", &format!("Invalid regular expression: /{}/{}: {}", pattern.to_rust_string(), flags, e.message))
}

/// Create a RegExp object from literal pattern/flags.
/// A regular expression literal: always the realm's %RegExp.prototype%.
/// (Not `new.target`: a literal evaluated inside a native constructor's
/// callback, e.g. a `new Promise(executor)`, must not pick up that
/// constructor's prototype.)
pub fn regexp_create(realm: &mut Realm, pattern: &str, flags: &str) -> JsResult<Value> {
    regexp_create_units(realm, &JsStr::new(pattern), flags)
}

/// RegExpCreate with a pattern that may hold lone surrogates.
pub fn regexp_create_units(realm: &mut Realm, pattern: &JsStr, flags: &str) -> JsResult<Value> {
    let proto = realm.intrinsics.others["RegExpPrototype"];
    regexp_alloc(realm, pattern, flags, proto, true)
}

fn regexp_alloc(realm: &mut Realm, pattern: &JsStr, flags: &str, proto: ObjectId, legacy: bool) -> JsResult<Value> {
    let regex = match compiled(pattern.as_units(), flags) {
        Ok(r) => r,
        Err(e) => return Err(syntax_error(realm, pattern, flags, &e)),
    };
    let data = RegExpData {
        regex,
        source: escape_source(pattern),
        flags: JsStr::new(flags),
        original_source: pattern.clone(),
        legacy,
        realm: realm.current_realm,
    };
    let id = realm.alloc(Object::new(Some(proto), ObjectKind::RegExp(Box::new(data))));
    realm.define_property(id, "lastIndex", Value::Number(0.0), WRITABLE);
    Ok(Value::Object(id))
}

/// EscapeRegExpPattern: a `source` that parses back to the same pattern
/// inside `/…/`.
fn escape_source(p: &JsStr) -> JsStr {
    let u = p.as_units();
    if u.is_empty() {
        return JsStr::new("(?:)");
    }
    let mut out: Vec<u16> = Vec::with_capacity(u.len() + 4);
    let mut class_depth = 0u32;
    let push_str = |out: &mut Vec<u16>, s: &str| out.extend(s.encode_utf16());
    let mut i = 0;
    while i < u.len() {
        let c = u[i];
        i += 1;
        match c {
            0x5C => {
                out.push(c);
                if i < u.len() {
                    let n = u[i];
                    i += 1;
                    match n {
                        0x0A => push_str(&mut out, "n"),
                        0x0D => push_str(&mut out, "r"),
                        0x2028 => push_str(&mut out, "u2028"),
                        0x2029 => push_str(&mut out, "u2029"),
                        _ => out.push(n),
                    }
                }
            }
            0x5B => {
                class_depth += 1;
                out.push(c);
            }
            0x5D => {
                class_depth = class_depth.saturating_sub(1);
                out.push(c);
            }
            0x2F if class_depth == 0 => push_str(&mut out, "\\/"),
            0x0A => push_str(&mut out, "\\n"),
            0x0D => push_str(&mut out, "\\r"),
            0x2028 => push_str(&mut out, "\\u2028"),
            0x2029 => push_str(&mut out, "\\u2029"),
            _ => out.push(c),
        }
    }
    JsStr::from_units(out)
}

/// RegExpCreate(P, F) (used by String.prototype.match / matchAll / search).
pub fn regexp_create_from(realm: &mut Realm, pattern: Value, flags: Value) -> JsResult<Value> {
    let proto = realm.intrinsics.others["RegExpPrototype"];
    let p = if pattern.is_undefined() { JsStr::empty() } else { realm.to_string(pattern)? };
    let f = if flags.is_undefined() { String::new() } else { realm.to_rust_string(flags)? };
    regexp_alloc(realm, &p, &f, proto, true)
}

fn this_regexp(realm: &mut Realm, this: &Value, method: &str) -> JsResult<ObjectId> {
    if let V::Object(id) = this.v() {
        if matches!(realm.obj(id).kind, ObjectKind::RegExp(_)) {
            return Ok(id);
        }
    }
    realm.type_error(&format!("RegExp.prototype.{} called on incompatible receiver", method))
}

/// IsRegExp(argument).
pub fn is_regexp(realm: &mut Realm, v: &Value) -> JsResult<bool> {
    if let V::Object(id) = v.v() {
        let m = realm.get(Value::Object(id), PropertyKey::Symbol(SYM_MATCH))?;
        if !m.is_undefined() {
            return Ok(m.truthy());
        }
        return Ok(matches!(realm.obj(id).kind, ObjectKind::RegExp(_)));
    }
    Ok(false)
}

fn regexp_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let pattern = arg(args, 0);
    let flags = arg(args, 1);
    let ctor = realm.intrinsics.others["RegExp"];
    let pattern_is_regexp = is_regexp(realm, &pattern)?;
    let mut new_target = realm.new_target();
    if new_target.is_undefined() {
        new_target = Value::Object(ctor);
        if pattern_is_regexp && flags.is_undefined() {
            let pc = realm.get_str(pattern.clone(), "constructor")?;
            if realm.same_value(&pc, &new_target) {
                return Ok(pattern);
            }
        }
    }
    let internal = match pattern.v() {
        V::Object(id) => match &realm.obj(id).kind {
            ObjectKind::RegExp(d) => Some((d.original_source.clone(), d.flags.clone())),
            _ => None,
        },
        _ => None,
    };
    let (p, f) = if let Some((src, fl)) = internal {
        (Value::String(src), if flags.is_undefined() { Value::String(fl) } else { flags })
    } else if pattern_is_regexp {
        let src = realm.get_str(pattern.clone(), "source")?;
        let fl = if flags.is_undefined() { realm.get_str(pattern.clone(), "flags")? } else { flags };
        (src, fl)
    } else {
        (pattern, flags)
    };
    // RegExpAlloc: the prototype comes from newTarget before the pattern
    // and flags are converted.
    let default_proto = realm.intrinsics.others["RegExpPrototype"];
    let proto = realm.proto_from_ctor(new_target.clone(), default_proto)?;
    let legacy = new_target.as_object() == Some(ctor);
    let p = if p.is_undefined() { JsStr::empty() } else { realm.to_string(p)? };
    let f = if f.is_undefined() { String::new() } else { realm.to_rust_string(f)? };
    regexp_alloc(realm, &p, &f, proto, legacy)
}

/// Annex B `RegExp.prototype.compile(pattern, flags)`.
fn compile(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_regexp(realm, &this, "compile")?;
    let cur = realm.current_realm;
    let legacy = matches!(&realm.obj(id).kind, ObjectKind::RegExp(d) if d.legacy && d.realm == cur);
    if !legacy {
        return realm.type_error("RegExp.prototype.compile called on a RegExp subclass instance");
    }
    let pattern = arg(args, 0);
    let flags = arg(args, 1);
    let internal = match pattern.v() {
        V::Object(pid) => match &realm.obj(pid).kind {
            ObjectKind::RegExp(d) => Some((d.original_source.clone(), d.flags.clone())),
            _ => None,
        },
        _ => None,
    };
    let (p, f) = match internal {
        Some((src, fl)) => {
            if !flags.is_undefined() {
                return realm.type_error("Cannot supply flags when constructing one RegExp from another");
            }
            (src, fl.to_rust_string())
        }
        None => {
            let p = if pattern.is_undefined() { JsStr::empty() } else { realm.to_string(pattern)? };
            let f = if flags.is_undefined() { String::new() } else { realm.to_rust_string(flags)? };
            (p, f)
        }
    };
    let regex = match compiled(p.as_units(), &f) {
        Ok(r) => r,
        Err(e) => return Err(syntax_error(realm, &p, &f, &e)),
    };
    if let ObjectKind::RegExp(d) = &mut realm.obj_mut(id).kind {
        d.regex = regex;
        d.source = escape_source(&p);
        d.flags = JsStr::new(&f);
        d.original_source = p;
    }
    set_last_index(realm, &this, 0)?;
    Ok(this)
}

fn get_last_index(realm: &mut Realm, rx: &Value) -> JsResult<usize> {
    let li = realm.get_str(rx.clone(), "lastIndex")?;
    realm.to_length(li)
}

fn set_last_index(realm: &mut Realm, rx: &Value, v: usize) -> JsResult<()> {
    set_last_index_value(realm, rx, Value::Number(v as f64))
}

fn set_last_index_value(realm: &mut Realm, rx: &Value, v: Value) -> JsResult<()> {
    let ok = realm.set_str(rx.clone(), "lastIndex", v)?;
    if !ok {
        return realm.type_error("Cannot assign to read only property 'lastIndex'");
    }
    Ok(())
}

/// The value of the named groups for a match: the first participating
/// group of each name.
fn group_value<T: Clone>(idxs: &[usize], per_group: &[Option<T>]) -> Option<T> {
    idxs.iter().find_map(|i| per_group.get(*i).cloned().flatten())
}

/// RegExpBuiltinExec: returns the match array or null.
pub fn builtin_exec(realm: &mut Realm, rx: ObjectId, s: &JsStr) -> JsResult<Value> {
    let rxv = Value::Object(rx);
    let li = realm.get_str(rxv.clone(), "lastIndex")?;
    let num = if let V::Number(n) = li.v() { Some(n) } else { None };
    let mut last_index = match num {
        Some(n) if (0.0..=9007199254740991.0).contains(&n) => n as usize,
        _ => realm.to_length(li)?,
    };
    let regex = regex_of(realm, rx);
    let (global, sticky, has_indices) = (regex.flags.global, regex.flags.sticky, regex.flags.has_indices);
    if !global && !sticky {
        last_index = 0;
    }
    if last_index > s.len() {
        if global || sticky {
            set_last_index(realm, &rxv, 0)?;
        }
        return Ok(Value::Null);
    }
    let Some(m) = regex.exec_utf16(s.as_units(), last_index) else {
        if global || sticky {
            set_last_index(realm, &rxv, 0)?;
        }
        return Ok(Value::Null);
    };
    if global || sticky {
        set_last_index(realm, &rxv, m.end())?;
    }
    realm.regexp_last = Some((s.clone(), m.captures.clone()));
    let caps: Vec<Value> = m
        .captures
        .iter()
        .map(|c| match c {
            Some((a, b)) => Value::String(s.slice(*a, *b)),
            None => Value::Undefined,
        })
        .collect();
    let arr = realm.new_array(caps.clone());
    realm.define_property(arr, "index", Value::Number(m.start() as f64), ALL);
    realm.define_property(arr, "input", Value::String(s.clone()), ALL);
    let names = regex.named_groups();
    let groups = if names.is_empty() {
        Value::Undefined
    } else {
        let g = realm.new_object_with_proto(None);
        let per_group: Vec<Option<Value>> = caps.iter().map(|v| if v.is_undefined() { None } else { Some(v.clone()) }).collect();
        for (name, idxs) in names {
            let v = group_value(idxs, &per_group).unwrap_or(Value::Undefined);
            realm.define_property(g, name.as_str(), v, ALL);
        }
        Value::Object(g)
    };
    realm.define_property(arr, "groups", groups, ALL);
    if has_indices {
        let mut pairs = Vec::with_capacity(m.captures.len());
        for c in &m.captures {
            pairs.push(match c {
                Some((a, b)) => realm.new_array_value(vec![Value::Number(*a as f64), Value::Number(*b as f64)]),
                None => Value::Undefined,
            });
        }
        let gi = if names.is_empty() {
            Value::Undefined
        } else {
            let g = realm.new_object_with_proto(None);
            let per_group: Vec<Option<Value>> = pairs.iter().map(|v| if v.is_undefined() { None } else { Some(v.clone()) }).collect();
            for (name, idxs) in names {
                let v = group_value(idxs, &per_group).unwrap_or(Value::Undefined);
                realm.define_property(g, name.as_str(), v, ALL);
            }
            Value::Object(g)
        };
        let indices = realm.new_array(pairs);
        realm.define_property(indices, "groups", gi, ALL);
        realm.define_property(arr, "indices", Value::Object(indices), ALL);
    }
    Ok(Value::Object(arr))
}

/// RegExpExec: honours a user-defined `exec`.
pub fn regexp_exec(realm: &mut Realm, rx: Value, s: &JsStr) -> JsResult<Value> {
    if let Some(id) = pristine(realm, &rx) {
        return builtin_exec(realm, id, s);
    }
    let exec_fn = realm.get_str(rx.clone(), "exec")?;
    if realm.is_callable(&exec_fn) {
        let builtin = realm.regexp_pristine.as_ref().map(|p| p.exec);
        let is_builtin = exec_fn.as_object().is_some() && exec_fn.as_object() == builtin;
        if !(is_builtin && matches!(rx.v(), V::Object(id) if matches!(realm.obj(id).kind, ObjectKind::RegExp(_)))) {
            let r = realm.call(exec_fn, rx, &[Value::String(s.clone())])?;
            if !r.is_object() && !r.is_null() {
                return realm.type_error("exec result must be an object or null");
            }
            return Ok(r);
        }
    }
    let id = this_regexp(realm, &rx, "exec")?;
    builtin_exec(realm, id, s)
}

fn exec(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_regexp(realm, &this, "exec")?;
    let s = realm.to_string(arg(args, 0))?;
    builtin_exec(realm, id, &s)
}

fn test(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if !this.is_object() {
        return realm.type_error("RegExp.prototype.test called on incompatible receiver");
    }
    let s = realm.to_string(arg(args, 0))?;
    if let Some(id) = pristine_plain(realm, &this) {
        // No result array needed: run the engine directly.
        let regex = regex_of(realm, id);
        let (global, sticky) = (regex.flags.global, regex.flags.sticky);
        let start = if global || sticky { get_last_index(realm, &this)? } else { 0 };
        let m = if start > s.len() { None } else { regex.exec_utf16(s.as_units(), start) };
        return Ok(Value::Bool(match m {
            Some(m) => {
                if global || sticky {
                    set_last_index(realm, &this, m.end())?;
                }
                realm.regexp_last = Some((s.clone(), m.captures));
                true
            }
            None => {
                if global || sticky {
                    set_last_index(realm, &this, 0)?;
                }
                false
            }
        }));
    }
    let r = regexp_exec(realm, this, &s)?;
    Ok(Value::Bool(!r.is_null()))
}

fn to_string(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    if !this.is_object() {
        return realm.type_error("RegExp.prototype.toString called on incompatible receiver");
    }
    let src = realm.get_str(this.clone(), "source")?;
    let src = realm.to_string(src)?;
    let fl = realm.get_str(this, "flags")?;
    let fl = realm.to_string(fl)?;
    let mut out: Vec<u16> = vec![b'/' as u16];
    out.extend_from_slice(src.as_units());
    out.push(b'/' as u16);
    out.extend_from_slice(fl.as_units());
    Ok(Value::String(JsStr::from_units(out)))
}

fn source(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    if !this.is_object() {
        return realm.type_error("RegExp.prototype.source getter called on non-object");
    }
    if this.as_object() == Some(realm.intrinsics.others["RegExpPrototype"]) {
        return Ok(Value::str("(?:)"));
    }
    let id = this_regexp(realm, &this, "source")?;
    Ok(match &realm.obj(id).kind {
        ObjectKind::RegExp(d) => Value::String(d.source.clone()),
        _ => unreachable!(),
    })
}

fn flags(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    if !this.is_object() {
        return realm.type_error("RegExp.prototype.flags getter called on non-object");
    }
    if let Some(id) = pristine(realm, &this) {
        return Ok(Value::str(&regex_of(realm, id).flags.to_string()));
    }
    let mut out = String::new();
    for (name, c) in [
        ("hasIndices", 'd'),
        ("global", 'g'),
        ("ignoreCase", 'i'),
        ("multiline", 'm'),
        ("dotAll", 's'),
        ("unicode", 'u'),
        ("unicodeSets", 'v'),
        ("sticky", 'y'),
    ] {
        if realm.get_str(this.clone(), name)?.truthy() {
            out.push(c);
        }
    }
    Ok(Value::str(&out))
}

fn flag_getter(realm: &mut Realm, this: Value, name: &str, f: fn(&crate::regex::Flags) -> bool) -> JsResult<Value> {
    if !this.is_object() {
        return realm.type_error(&format!("RegExp.prototype.{name} getter called on non-object"));
    }
    if this.as_object() == Some(realm.intrinsics.others["RegExpPrototype"]) {
        return Ok(Value::Undefined);
    }
    let id = this_regexp(realm, &this, name)?;
    Ok(Value::Bool(f(&regex_of(realm, id).flags)))
}

fn flag_global(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    flag_getter(realm, this, "global", |f| f.global)
}
fn flag_ignore_case(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    flag_getter(realm, this, "ignoreCase", |f| f.ignore_case)
}
fn flag_multiline(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    flag_getter(realm, this, "multiline", |f| f.multiline)
}
fn flag_dot_all(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    flag_getter(realm, this, "dotAll", |f| f.dot_all)
}
fn flag_unicode(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    flag_getter(realm, this, "unicode", |f| f.unicode)
}
fn flag_unicode_sets(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    flag_getter(realm, this, "unicodeSets", |f| f.unicode_sets)
}
fn flag_sticky(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    flag_getter(realm, this, "sticky", |f| f.sticky)
}
fn flag_has_indices(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    flag_getter(realm, this, "hasIndices", |f| f.has_indices)
}

/// AdvanceStringIndex.
fn advance_index(s: &JsStr, index: usize, unicode: bool) -> usize {
    if !unicode || index + 1 >= s.len() {
        return index + 1;
    }
    let u = s.as_units();
    if (0xD800..0xDC00).contains(&u[index]) && (0xDC00..0xE000).contains(&u[index + 1]) {
        index + 2
    } else {
        index + 1
    }
}

fn flags_string(realm: &mut Realm, rx: &Value) -> JsResult<String> {
    let flags = realm.get_str(rx.clone(), "flags")?;
    realm.to_rust_string(flags)
}

fn symbol_match(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if !this.is_object() {
        return realm.type_error("RegExp.prototype[Symbol.match] called on non-object");
    }
    let s = realm.to_string(arg(args, 0))?;
    if let Some(id) = pristine_plain(realm, &this) {
        let regex = regex_of(realm, id);
        if regex.flags.global {
            // All matches straight from the engine.
            set_last_index(realm, &this, 0)?;
            let unicode = regex.flags.either_unicode();
            let mut results = Vec::new();
            let mut pos = 0;
            while pos <= s.len() {
                let Some(m) = regex.exec_utf16(s.as_units(), pos) else { break };
                let (a, b) = (m.start(), m.end());
                results.push(Value::String(s.slice(a, b)));
                realm.regexp_last = Some((s.clone(), m.captures));
                pos = if a == b { advance_index(&s, b, unicode) } else { b };
            }
            set_last_index(realm, &this, 0)?;
            if results.is_empty() {
                return Ok(Value::Null);
            }
            return Ok(realm.new_array_value(results));
        }
    }
    let flags = flags_string(realm, &this)?;
    if !flags.contains('g') {
        return regexp_exec(realm, this, &s);
    }
    let unicode = flags.contains('u') || flags.contains('v');
    set_last_index(realm, &this, 0)?;
    let mut results = Vec::new();
    loop {
        let r = regexp_exec(realm, this.clone(), &s)?;
        if r.is_null() {
            break;
        }
        let m = realm.get(r, PropertyKey::index(0))?;
        let ms = realm.to_string(m)?;
        if ms.is_empty() {
            let li = get_last_index(realm, &this)?;
            set_last_index(realm, &this, advance_index(&s, li, unicode))?;
        }
        results.push(Value::String(ms));
    }
    if results.is_empty() {
        return Ok(Value::Null);
    }
    Ok(realm.new_array_value(results))
}

fn symbol_match_all(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(rid) = this.as_object() else {
        return realm.type_error("RegExp.prototype[Symbol.matchAll] called on non-object");
    };
    let s = realm.to_string(arg(args, 0))?;
    let default = realm.intrinsics.others["RegExp"];
    let c = realm.species_constructor(rid, default)?;
    let flags = flags_string(realm, &this)?;
    let matcher = realm.construct(c, &[this.clone(), Value::str(&flags)], Value::Undefined)?;
    let li = get_last_index(realm, &this)?;
    set_last_index(realm, &matcher, li)?;
    let Some(matcher) = matcher.as_object() else {
        return realm.type_error("RegExp species constructor returned a non-object");
    };
    let proto = realm.intrinsics.others["RegExpStringIteratorPrototype"];
    let it = realm.alloc(Object::new(
        Some(proto),
        ObjectKind::RegExpStringIterator {
            regexp: matcher,
            string: s,
            global: flags.contains('g'),
            unicode: flags.contains('u') || flags.contains('v'),
            done: false,
        },
    ));
    Ok(Value::Object(it))
}

fn regexp_string_iterator_next(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let id = this_object(realm, &this, "RegExp String Iterator.prototype.next")?;
    let (rx, s, global, unicode, done) = match &realm.obj(id).kind {
        ObjectKind::RegExpStringIterator { regexp, string, global, unicode, done } => (*regexp, string.clone(), *global, *unicode, *done),
        _ => return realm.type_error("next called on incompatible receiver"),
    };
    if done {
        return Ok(realm.create_iter_result(Value::Undefined, true));
    }
    let r = regexp_exec(realm, Value::Object(rx), &s)?;
    if r.is_null() {
        if let ObjectKind::RegExpStringIterator { done, .. } = &mut realm.obj_mut(id).kind {
            *done = true;
        }
        return Ok(realm.create_iter_result(Value::Undefined, true));
    }
    if !global {
        if let ObjectKind::RegExpStringIterator { done, .. } = &mut realm.obj_mut(id).kind {
            *done = true;
        }
        return Ok(realm.create_iter_result(r, false));
    }
    let m = realm.get(r.clone(), PropertyKey::index(0))?;
    let ms = realm.to_string(m)?;
    if ms.is_empty() {
        let li = get_last_index(realm, &Value::Object(rx))?;
        set_last_index(realm, &Value::Object(rx), advance_index(&s, li, unicode))?;
    }
    Ok(realm.create_iter_result(r, false))
}

fn symbol_replace(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if !this.is_object() {
        return realm.type_error("RegExp.prototype[Symbol.replace] called on non-object");
    }
    let s = realm.to_string(arg(args, 0))?;
    let rep = arg(args, 1);
    let functional = realm.is_callable(&rep);
    let rep_str = if functional { JsStr::empty() } else { realm.to_string(rep.clone())? };
    if let Some(rx) = pristine_plain(realm, &this) {
        if let Some(v) = fast_replace(realm, rx, &s, &rep, functional, &rep_str)? {
            return Ok(v);
        }
    }
    let flags = flags_string(realm, &this)?;
    let global = flags.contains('g');
    let unicode = flags.contains('u') || flags.contains('v');
    if global {
        set_last_index(realm, &this, 0)?;
    }
    let mut results = Vec::new();
    loop {
        let r = regexp_exec(realm, this.clone(), &s)?;
        if r.is_null() {
            break;
        }
        results.push(r.clone());
        if !global {
            break;
        }
        let m = realm.get(r, PropertyKey::index(0))?;
        let ms = realm.to_string(m)?;
        if ms.is_empty() {
            let li = get_last_index(realm, &this)?;
            set_last_index(realm, &this, advance_index(&s, li, unicode))?;
        }
    }
    let mut out: Vec<u16> = Vec::new();
    let mut next_pos = 0usize;
    for r in results {
        let ncaps = realm.length_of(r.clone())?.max(1) - 1;
        let matched = realm.get(r.clone(), PropertyKey::index(0))?;
        let matched = realm.to_string(matched)?;
        let pos_v = realm.get_str(r.clone(), "index")?;
        let position = to_integer(realm.to_number(pos_v)?).max(0.0).min(s.len() as f64) as usize;
        let mut captures = Vec::with_capacity(ncaps);
        for i in 1..=ncaps {
            let c = realm.get(r.clone(), PropertyKey::index(i as u32))?;
            captures.push(if c.is_undefined() { c } else { Value::String(realm.to_string(c)?) });
        }
        let named = realm.get_str(r.clone(), "groups")?;
        let replacement = if functional {
            let mut fargs = vec![Value::String(matched.clone())];
            fargs.extend(captures.iter().cloned());
            fargs.push(Value::Number(position as f64));
            fargs.push(Value::String(s.clone()));
            if !named.is_undefined() {
                fargs.push(named.clone());
            }
            let rv = realm.call(rep.clone(), Value::Undefined, &fargs)?;
            realm.to_string(rv)?
        } else {
            let named = if named.is_undefined() { Value::Undefined } else { Value::Object(realm.to_object(named)?) };
            string::get_substitution(realm, &matched, &s, position, &captures, &named, &rep_str)?
        };
        if position >= next_pos {
            out.extend_from_slice(&s.as_units()[next_pos..position]);
            out.extend_from_slice(replacement.as_units());
            next_pos = position + matched.len();
        }
    }
    if next_pos < s.len() {
        out.extend_from_slice(&s.as_units()[next_pos..]);
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn symbol_search(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if !this.is_object() {
        return realm.type_error("RegExp.prototype[Symbol.search] called on non-object");
    }
    let s = realm.to_string(arg(args, 0))?;
    let prev = realm.get_str(this.clone(), "lastIndex")?;
    let zero = Value::Number(0.0);
    if !realm.same_value(&prev, &zero) {
        set_last_index_value(realm, &this, zero)?;
    }
    let r = regexp_exec(realm, this.clone(), &s)?;
    let cur = realm.get_str(this.clone(), "lastIndex")?;
    if !realm.same_value(&cur, &prev) {
        set_last_index_value(realm, &this, prev)?;
    }
    if r.is_null() {
        return Ok(Value::Number(-1.0));
    }
    realm.get_str(r, "index")
}

fn symbol_split(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let Some(rid) = this.as_object() else {
        return realm.type_error("RegExp.prototype[Symbol.split] called on non-object");
    };
    let s = realm.to_string(arg(args, 0))?;
    let default = realm.intrinsics.others["RegExp"];
    let c = realm.species_constructor(rid, default)?;
    let fast = if c.as_object() == Some(default) { pristine(realm, &this) } else { None };
    let flags = flags_string(realm, &this)?;
    let unicode = flags.contains('u') || flags.contains('v');
    let new_flags = if flags.contains('y') { flags.clone() } else { format!("{}y", flags) };
    let mut splitter = if fast.is_some() { Value::Undefined } else { realm.construct(c, &[this.clone(), Value::str(&new_flags)], Value::Undefined)? };
    // The splitter's matcher is fixed now (ToUint32(limit) may recompile `this`).
    let saved = fast.map(|id| match &realm.obj(id).kind {
        ObjectKind::RegExp(d) => (d.regex.clone(), d.original_source.clone()),
        _ => unreachable!(),
    });
    let limit = arg(args, 1);
    let lim = if limit.is_undefined() { u32::MAX as usize } else { realm.to_uint32(limit)? as usize };
    let mut fast_regex = None;
    if let Some((regex, src)) = saved {
        // ToUint32(limit) may also have changed RegExp.prototype: then
        // materialize the splitter the spec would have built.
        if pristine(realm, &this).is_some() {
            fast_regex = Some(regex);
        } else {
            let proto = realm.intrinsics.others["RegExpPrototype"];
            splitter = regexp_alloc(realm, &src, &new_flags, proto, true)?;
        }
    }
    let mut parts: Vec<Value> = Vec::new();
    if lim == 0 {
        return Ok(realm.new_array_value(parts));
    }
    if let Some(regex) = fast_regex {
        // %RegExp% splitter over a pristine regexp: search with the engine
        // instead of a sticky exec at every position.
        let size = s.len();
        if size == 0 {
            if regex.search_utf16(s.as_units(), 0, true).is_none() {
                parts.push(Value::String(s));
            }
            return Ok(realm.new_array_value(parts));
        }
        let mut p = 0usize;
        let mut q = 0usize;
        while q < size {
            let Some(m) = regex.search_utf16(s.as_units(), q, false) else { break };
            if m.start() >= size {
                break;
            }
            let e = m.end().min(size);
            if e == p {
                q = advance_index(&s, m.start(), unicode);
                continue;
            }
            parts.push(Value::String(s.slice(p, m.start())));
            if parts.len() >= lim {
                return Ok(realm.new_array_value(parts));
            }
            p = e;
            for c in m.captures.iter().skip(1) {
                parts.push(match c {
                    Some((a, b)) => Value::String(s.slice(*a, *b)),
                    None => Value::Undefined,
                });
                if parts.len() >= lim {
                    return Ok(realm.new_array_value(parts));
                }
            }
            realm.regexp_last = Some((s.clone(), m.captures));
            q = p;
        }
        parts.push(Value::String(s.slice(p, size)));
        return Ok(realm.new_array_value(parts));
    }
    if s.is_empty() {
        let z = regexp_exec(realm, splitter, &s)?;
        if z.is_null() {
            parts.push(Value::String(s));
        }
        return Ok(realm.new_array_value(parts));
    }
    let size = s.len();
    let mut p = 0usize;
    let mut q = p;
    while q < size {
        set_last_index(realm, &splitter, q)?;
        let z = regexp_exec(realm, splitter.clone(), &s)?;
        if z.is_null() {
            q = advance_index(&s, q, unicode);
            continue;
        }
        let e = get_last_index(realm, &splitter)?.min(size);
        if e == p {
            q = advance_index(&s, q, unicode);
            continue;
        }
        parts.push(Value::String(s.slice(p, q)));
        if parts.len() >= lim {
            return Ok(realm.new_array_value(parts));
        }
        p = e;
        let ncaps = realm.length_of(z.clone())?.max(1) - 1;
        for i in 1..=ncaps {
            let c = realm.get(z.clone(), PropertyKey::index(i as u32))?;
            parts.push(c);
            if parts.len() >= lim {
                return Ok(realm.new_array_value(parts));
            }
        }
        q = p;
    }
    parts.push(Value::String(s.slice(p, size)));
    Ok(realm.new_array_value(parts))
}

/// `RegExp.escape(string)`.
fn regexp_escape(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let s = match arg(args, 0).v() {
        V::String(s) => s.clone(),
        _ => return realm.type_error("RegExp.escape requires a string"),
    };
    let u = s.as_units();
    let mut out: Vec<u16> = Vec::with_capacity(u.len() * 2);
    let push = |out: &mut Vec<u16>, s: &str| out.extend(s.encode_utf16());
    let mut i = 0;
    while i < u.len() {
        let (cp, len) = if (0xD800..0xDC00).contains(&u[i]) && i + 1 < u.len() && (0xDC00..0xE000).contains(&u[i + 1]) {
            (0x10000 + (((u[i] as u32) - 0xD800) << 10) + ((u[i + 1] as u32) - 0xDC00), 2)
        } else {
            (u[i] as u32, 1)
        };
        let ch = char::from_u32(cp);
        if out.is_empty() && matches!(ch, Some(c) if c.is_ascii_alphanumeric()) {
            push(&mut out, &format!("\\x{:02x}", cp));
        } else if matches!(ch, Some('^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '/')) {
            out.push(b'\\' as u16);
            out.push(cp as u16);
        } else if let Some(e) = match cp {
            0x09 => Some("\\t"),
            0x0A => Some("\\n"),
            0x0B => Some("\\v"),
            0x0C => Some("\\f"),
            0x0D => Some("\\r"),
            _ => None,
        } {
            push(&mut out, e);
        } else if matches!(ch, Some(',' | '-' | '=' | '<' | '>' | '#' | '&' | '!' | '%' | ':' | ';' | '@' | '~' | '\'' | '`' | '"'))
            || matches!(cp, 0x20 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000 | 0xFEFF | 0xD800..=0xDFFF)
        {
            if cp <= 0xFF {
                push(&mut out, &format!("\\x{:02x}", cp));
            } else {
                for &cu in &u[i..i + len] {
                    push(&mut out, &format!("\\u{:04x}", cu));
                }
            }
        } else {
            out.extend_from_slice(&u[i..i + len]);
        }
        i += len;
    }
    Ok(Value::String(JsStr::from_units(out)))
}

#[cfg(test)]
mod pristine_tests {
    use super::*;

    #[test]
    fn literal_regexps_are_pristine() {
        let mut realm = Realm::new();
        assert!(realm.regexp_pristine.is_some());
        let v = realm.eval("/-/g", "t.js").unwrap();
        assert!(pristine(&realm, &v).is_some());
        let v = realm.eval("var r = /x/; r.foo = 1; r", "t.js").unwrap();
        assert!(pristine(&realm, &v).is_none());
    }
}
