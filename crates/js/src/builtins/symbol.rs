//! Symbol constructor and prototype.

use super::*;
use crate::value::V;
use crate::realm::{SYM_TO_PRIMITIVE, SYM_TO_STRING_TAG, WELL_KNOWN_SYMBOLS};

pub fn register(realm: &mut Realm) {
    let sp = realm.intrinsics.symbol_proto;
    realm.define_method(sp, "toString", 0, to_string);
    realm.define_method(sp, "valueOf", 0, value_of);
    realm.define_accessor(sp, "description", Some(description), None, CONFIGURABLE);
    let tp = realm.new_native_function("[Symbol.toPrimitive]", 1, value_of);
    realm.define_property(sp, PropertyKey::Symbol(SYM_TO_PRIMITIVE), Value::Object(tp), CONFIGURABLE);
    realm.define_property(sp, PropertyKey::Symbol(SYM_TO_STRING_TAG), Value::str("Symbol"), CONFIGURABLE);
    let ctor = define_ctor(realm, "Symbol", 0, symbol_ctor, sp);
    realm.define_method(ctor, "for", 1, symbol_for);
    realm.define_method(ctor, "keyFor", 1, key_for);
    for (name, id) in WELL_KNOWN_SYMBOLS.iter() {
        realm.define_property(ctor, *name, Value::Symbol(*id), 0);
    }
}

fn symbol_ctor(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    if realm.is_construct_call() {
        return realm.type_error("Symbol is not a constructor");
    }
    let desc = match arg(args, 0) {
        Value::Undefined => None,
        v => Some(realm.to_string(v)?),
    };
    Ok(Value::Symbol(realm.new_symbol(desc)))
}

fn this_symbol(realm: &mut Realm, this: &Value) -> JsResult<SymbolId> {
    match this.v() {
        V::Symbol(s) => Ok(s),
        V::Object(id) => match &realm.obj(id).kind {
            ObjectKind::Symbol(s) => Ok(*s),
            _ => realm.type_error("not a Symbol"),
        },
        _ => realm.type_error("not a Symbol"),
    }
}

fn to_string(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let s = this_symbol(realm, &this)?;
    Ok(Value::str(&format!("Symbol({})", realm.symbol_description(s))))
}

fn value_of(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    Ok(Value::Symbol(this_symbol(realm, &this)?))
}

fn description(realm: &mut Realm, this: Value, _args: &[Value]) -> JsResult<Value> {
    let s = this_symbol(realm, &this)?;
    Ok(match &realm.symbols[s.0 as usize].description {
        Some(d) => Value::String(d.clone()),
        None => Value::Undefined,
    })
}

fn symbol_for(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    let key = realm.to_string(arg(args, 0))?;
    Ok(Value::Symbol(realm.symbol_for(key)))
}

fn key_for(realm: &mut Realm, _this: Value, args: &[Value]) -> JsResult<Value> {
    match (arg(args, 0)).v() {
        V::Symbol(s) => Ok(realm.symbol_key_for(s).map(Value::String).unwrap_or(Value::Undefined)),
        _ => realm.type_error("Symbol.keyFor: argument is not a symbol"),
    }
}
