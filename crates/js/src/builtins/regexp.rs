//! RegExp objects: construction, exec/test and the Symbol.match/replace/
//! search/split/matchAll protocol used by String.prototype.

use super::*;
use crate::realm::{SYM_MATCH, SYM_MATCH_ALL, SYM_REPLACE, SYM_SEARCH, SYM_SPECIES, SYM_SPLIT, SYM_TO_STRING_TAG};
use crate::regex::Regex;

pub fn register(realm: &mut Realm) {
    let rp = realm.new_object();
    realm.intrinsics.others.insert("RegExpPrototype", rp);
    let ctor = define_ctor(realm, "RegExp", 2, regexp_ctor, rp);
    realm.intrinsics.others.insert("RegExp", ctor);
    realm.define_accessor(ctor, PropertyKey::Symbol(SYM_SPECIES), Some(species), None, CONFIGURABLE);
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
}

fn species(_realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(this)
}

/// Create a RegExp object from literal pattern/flags.
pub fn regexp_create(realm: &mut Realm, pattern: &str, flags: &str) -> JsResult<Value> {
    let default_proto = realm.intrinsics.others["RegExpPrototype"];
    let proto = realm.proto_from_new_target(default_proto);
    regexp_alloc(realm, pattern, flags, proto)
}

fn regexp_alloc(realm: &mut Realm, pattern: &str, flags: &str, proto: ObjectId) -> JsResult<Value> {
    let regex = match Regex::new(pattern, flags) {
        Ok(r) => r,
        Err(e) => return Err(realm.new_error("SyntaxError", &format!("Invalid regular expression: /{}/{}: {}", pattern, flags, e.message))),
    };
    let source = if pattern.is_empty() { "(?:)".to_string() } else { escape_source(pattern) };
    let id = realm.alloc(Object::new(Some(proto), ObjectKind::RegExp(Box::new(RegExpData { regex, source: JsStr::new(&source), flags: JsStr::new(&canonical_flags(flags)) }))));
    realm.define_property(id, "lastIndex", Value::Number(0.0), WRITABLE);
    Ok(Value::Object(id))
}

fn canonical_flags(flags: &str) -> String {
    let mut out = String::new();
    for c in ['d', 'g', 'i', 'm', 's', 'u', 'v', 'y'] {
        if flags.contains(c) {
            out.push(c);
        }
    }
    out
}

fn escape_source(p: &str) -> String {
    let mut out = String::new();
    let mut in_class = false;
    let mut chars = p.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push('\\');
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            '[' => {
                in_class = true;
                out.push(c);
            }
            ']' => {
                in_class = false;
                out.push(c);
            }
            '/' if !in_class => out.push_str("\\/"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

/// `new RegExp(pattern, flags)` semantics for String methods.
pub fn regexp_create_from(realm: &mut Realm, pattern: Value, flags: Value) -> JsResult<Value> {
    let ctor = Value::Object(realm.intrinsics.others["RegExp"]);
    realm.construct(ctor, &[pattern, flags], Value::Undefined)
}

fn this_regexp(realm: &mut Realm, this: &Value, method: &str) -> JsResult<ObjectId> {
    if let Value::Object(id) = this {
        if matches!(realm.obj(*id).kind, ObjectKind::RegExp(_)) {
            return Ok(*id);
        }
    }
    realm.type_error(&format!("RegExp.prototype.{} called on incompatible receiver", method))
}

fn regexp_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let pattern = arg(args, 0);
    let flags = arg(args, 1);
    let pattern_is_regexp = matches!(&pattern, Value::Object(id) if matches!(realm.obj(*id).kind, ObjectKind::RegExp(_)));
    if !realm.is_construct_call() && pattern_is_regexp && flags.is_undefined() {
        let ctor = realm.get_str(pattern.clone(), "constructor")?;
        if ctor.as_object() == Some(realm.intrinsics.others["RegExp"]) {
            return Ok(pattern);
        }
    }
    let (p, f) = if pattern_is_regexp {
        let id = pattern.as_object().unwrap();
        let (src, fl) = match &realm.obj(id).kind {
            ObjectKind::RegExp(d) => (d.regex.source.clone(), d.flags.to_rust_string()),
            _ => unreachable!(),
        };
        let f = if flags.is_undefined() { fl } else { realm.to_rust_string(flags)? };
        (src, f)
    } else {
        let p = if pattern.is_undefined() { String::new() } else { realm.to_rust_string(pattern)? };
        let f = if flags.is_undefined() { String::new() } else { realm.to_rust_string(flags)? };
        (p, f)
    };
    let default_proto = realm.intrinsics.others["RegExpPrototype"];
    let proto = realm.proto_from_new_target(default_proto);
    regexp_alloc(realm, &p, &f, proto)
}

fn compile(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    let id = this_regexp(realm, &this, "compile")?;
    let p = if arg(args, 0).is_undefined() { String::new() } else { realm.to_rust_string(arg(args, 0))? };
    let f = if arg(args, 1).is_undefined() { String::new() } else { realm.to_rust_string(arg(args, 1))? };
    let regex = match Regex::new(&p, &f) {
        Ok(r) => r,
        Err(e) => return Err(realm.new_error("SyntaxError", &e.message)),
    };
    if let ObjectKind::RegExp(d) = &mut realm.obj_mut(id).kind {
        d.regex = regex;
        d.source = JsStr::new(&if p.is_empty() { "(?:)".to_string() } else { escape_source(&p) });
        d.flags = JsStr::new(&canonical_flags(&f));
    }
    realm.set_str(this.clone(), "lastIndex", Value::Number(0.0))?;
    Ok(this)
}

fn get_last_index(realm: &mut Realm, rx: &Value) -> JsResult<usize> {
    let li = realm.get_str(rx.clone(), "lastIndex")?;
    realm.to_length(li)
}

fn set_last_index(realm: &mut Realm, rx: &Value, v: usize) -> JsResult<()> {
    let ok = realm.set_str(rx.clone(), "lastIndex", Value::Number(v as f64))?;
    if !ok {
        return realm.type_error("Cannot assign to read only property 'lastIndex'");
    }
    Ok(())
}

/// RegExpBuiltinExec: returns the match array or null.
pub fn builtin_exec(realm: &mut Realm, rx: ObjectId, s: &JsStr) -> JsResult<Value> {
    let (global, sticky, has_indices) = match &realm.obj(rx).kind {
        ObjectKind::RegExp(d) => (d.regex.flags.global, d.regex.flags.sticky, d.regex.flags.has_indices),
        _ => unreachable!(),
    };
    let rxv = Value::Object(rx);
    let mut last_index = if global || sticky { get_last_index(realm, &rxv)? } else { 0 };
    if last_index > s.len() {
        if global || sticky {
            set_last_index(realm, &rxv, 0)?;
        }
        return Ok(Value::Null);
    }
    let m = {
        let d = match &realm.obj(rx).kind {
            ObjectKind::RegExp(d) => d,
            _ => unreachable!(),
        };
        d.regex.exec_utf16(s.as_units(), last_index)
    };
    let m = match m {
        Some(m) => m,
        None => {
            if global || sticky {
                set_last_index(realm, &rxv, 0)?;
            }
            return Ok(Value::Null);
        }
    };
    if sticky && m.start() != last_index {
        set_last_index(realm, &rxv, 0)?;
        return Ok(Value::Null);
    }
    last_index = m.end();
    if global || sticky {
        set_last_index(realm, &rxv, last_index)?;
    }
    let names: Vec<(String, usize)> = match &realm.obj(rx).kind {
        ObjectKind::RegExp(d) => d.regex.group_names().to_vec(),
        _ => unreachable!(),
    };
    let mut caps: Vec<Value> = Vec::with_capacity(m.captures.len());
    for c in &m.captures {
        caps.push(match c {
            Some((a, b)) => Value::String(s.slice(*a, *b)),
            None => Value::Undefined,
        });
    }
    let arr = realm.new_array(caps.clone());
    realm.define_property(arr, "index", Value::Number(m.start() as f64), ALL);
    realm.define_property(arr, "input", Value::String(s.clone()), ALL);
    let groups = if names.is_empty() {
        Value::Undefined
    } else {
        let g = realm.new_object_with_proto(None);
        for (name, idx) in &names {
            let v = caps.get(*idx).cloned().unwrap_or(Value::Undefined);
            realm.define_property(g, name.as_str(), v, ALL);
        }
        Value::Object(g)
    };
    realm.define_property(arr, "groups", groups.clone(), ALL);
    if has_indices {
        let mut pairs = Vec::new();
        for c in &m.captures {
            pairs.push(match c {
                Some((a, b)) => realm.new_array_value(vec![Value::Number(*a as f64), Value::Number(*b as f64)]),
                None => Value::Undefined,
            });
        }
        let indices = realm.new_array(pairs);
        let gi = if names.is_empty() {
            Value::Undefined
        } else {
            let g = realm.new_object_with_proto(None);
            for (name, idx) in &names {
                let v = match m.captures.get(*idx).copied().flatten() {
                    Some((a, b)) => realm.new_array_value(vec![Value::Number(a as f64), Value::Number(b as f64)]),
                    None => Value::Undefined,
                };
                realm.define_property(g, name.as_str(), v, ALL);
            }
            Value::Object(g)
        };
        realm.define_property(indices, "groups", gi, ALL);
        realm.define_property(arr, "indices", Value::Object(indices), ALL);
    }
    Ok(Value::Object(arr))
}

/// RegExpExec: honours a user-defined `exec`.
pub fn regexp_exec(realm: &mut Realm, rx: Value, s: &JsStr) -> JsResult<Value> {
    let exec_fn = realm.get_str(rx.clone(), "exec")?;
    if realm.is_callable(&exec_fn) {
        let is_builtin = exec_fn.as_object().map(|f| realm.function_name(f).eq_str("exec") && matches!(realm.obj(f).kind, ObjectKind::Native(_))).unwrap_or(false);
        if !is_builtin {
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
    let r = regexp_exec(realm, this, &s)?;
    Ok(Value::Bool(!r.is_null()))
}

fn to_string(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    if !this.is_object() {
        return realm.type_error("RegExp.prototype.toString called on incompatible receiver");
    }
    let src = realm.get_str(this.clone(), "source")?;
    let src = realm.to_rust_string(src)?;
    let fl = realm.get_str(this, "flags")?;
    let fl = realm.to_rust_string(fl)?;
    Ok(Value::str(&format!("/{}/{}", src, fl)))
}

fn source(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
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
    let mut out = String::new();
    for (name, c) in [("hasIndices", 'd'), ("global", 'g'), ("ignoreCase", 'i'), ("multiline", 'm'), ("dotAll", 's'), ("unicode", 'u'), ("unicodeSets", 'v'), ("sticky", 'y')] {
        if realm.get_str(this.clone(), name)?.truthy() {
            out.push(c);
        }
    }
    Ok(Value::str(&out))
}

fn flag_getter(realm: &mut Realm, this: Value, f: fn(&crate::regex::Flags) -> bool) -> JsResult<Value> {
    if this.as_object() == Some(realm.intrinsics.others["RegExpPrototype"]) {
        return Ok(Value::Undefined);
    }
    let id = this_regexp(realm, &this, "flag")?;
    Ok(match &realm.obj(id).kind {
        ObjectKind::RegExp(d) => Value::Bool(f(&d.regex.flags)),
        _ => unreachable!(),
    })
}

fn flag_global(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    flag_getter(realm, this, |f| f.global)
}
fn flag_ignore_case(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    flag_getter(realm, this, |f| f.ignore_case)
}
fn flag_multiline(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    flag_getter(realm, this, |f| f.multiline)
}
fn flag_dot_all(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    flag_getter(realm, this, |f| f.dot_all)
}
fn flag_unicode(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    flag_getter(realm, this, |f| f.unicode)
}
fn flag_sticky(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    flag_getter(realm, this, |f| f.sticky)
}
fn flag_has_indices(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    flag_getter(realm, this, |f| f.has_indices)
}

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

fn symbol_match(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if !this.is_object() {
        return realm.type_error("RegExp.prototype[Symbol.match] called on non-object");
    }
    let s = realm.to_string(arg(args, 0))?;
    let flags = realm.get_str(this.clone(), "flags")?;
    let flags = realm.to_rust_string(flags)?;
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
    if !this.is_object() {
        return realm.type_error("RegExp.prototype[Symbol.matchAll] called on non-object");
    }
    let s = realm.to_string(arg(args, 0))?;
    let flags = realm.get_str(this.clone(), "flags")?;
    let flags = realm.to_rust_string(flags)?;
    let matcher = regexp_create_from(realm, this.clone(), Value::str(&flags))?;
    let li = get_last_index(realm, &this)?;
    set_last_index(realm, &matcher, li)?;
    let proto = realm.intrinsics.others["RegExpStringIteratorPrototype"];
    let it = realm.alloc(Object::new(Some(proto), ObjectKind::RegExpStringIterator { regexp: matcher.as_object().unwrap(), string: s, global: flags.contains('g'), unicode: flags.contains('u'), done: false }));
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
    let flags = realm.get_str(this.clone(), "flags")?;
    let flags = realm.to_rust_string(flags)?;
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
        let position = (to_integer(realm.to_number(pos_v)?).max(0.0) as usize).min(s.len());
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
    set_last_index(realm, &this, 0)?;
    let r = regexp_exec(realm, this.clone(), &s)?;
    realm.set_str(this, "lastIndex", prev)?;
    if r.is_null() {
        return Ok(Value::Number(-1.0));
    }
    realm.get_str(r, "index")
}

fn symbol_split(realm: &mut Realm, this: Value, args: &[Value]) -> JsResult<Value> {
    if !this.is_object() {
        return realm.type_error("RegExp.prototype[Symbol.split] called on non-object");
    }
    let s = realm.to_string(arg(args, 0))?;
    let flags = realm.get_str(this.clone(), "flags")?;
    let flags = realm.to_rust_string(flags)?;
    let unicode = flags.contains('u') || flags.contains('v');
    let new_flags = if flags.contains('y') { flags.clone() } else { format!("{}y", flags) };
    let splitter = regexp_create_from(realm, this, Value::str(&new_flags))?;
    let limit = arg(args, 1);
    let lim = if limit.is_undefined() { u32::MAX as usize } else { realm.to_uint32(limit)? as usize };
    let mut parts: Vec<Value> = Vec::new();
    if lim == 0 {
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
