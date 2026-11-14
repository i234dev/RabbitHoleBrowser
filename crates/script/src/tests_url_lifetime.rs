//! URL and its SameObject query list retain one another only while reachable.
use crate::tests::{run, setup};
use crate::{js::{ObjectId, Value}, ScriptHost};

fn global_object(host: &mut ScriptHost, name: &str) -> ObjectId {
    let realm = host.realm();
    let global = realm.global();
    realm.get_str(Value::Object(global), name).unwrap().as_object().unwrap()
}

#[test]
fn live_url_keeps_its_cached_query_list_through_collection() {
    let (mut env, mut host) = setup("<body></body>");
    run(&mut env, &mut host, "globalThis.savedUrl = new URL('https://example.test/?initial=1'); void savedUrl.searchParams;");
    let url = global_object(&mut host, "savedUrl");
    let params = host.realm().host_data::<crate::fetch::UrlData>(url).unwrap().params.unwrap();
    host.gc();
    assert_eq!(run(&mut env, &mut host, "savedUrl.search='?after=two'; console.log(savedUrl.searchParams.get('after')); savedUrl.href='https://example.test/next?fresh=three'; console.log(savedUrl.searchParams.get('fresh'));"), ["two", "three"]);
    assert_eq!(host.realm().host_data::<crate::fetch::UrlData>(url).unwrap().params, Some(params));
    assert!(host.realm().heap.try_get(params).is_some());
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}

#[test]
fn live_query_list_keeps_its_url_and_updates_it_after_collection() {
    let (mut env, mut host) = setup("<body></body>");
    run(&mut env, &mut host, "globalThis.savedParams = new URL('https://example.test/?initial=1').searchParams;");
    let params = global_object(&mut host, "savedParams");
    let url = host.realm().host_data::<crate::fetch::ParamsData>(params).unwrap().owner.unwrap();
    host.gc();
    assert_eq!(run(&mut env, &mut host, "savedParams.append('b','2'); savedParams.set('a','1'); savedParams.delete('initial'); savedParams.sort(); console.log(savedParams.toString());"), ["a=1&b=2"]);
    assert_eq!(host.realm().host_data::<crate::fetch::UrlData>(url).unwrap().url.search(), "?a=1&b=2");
    assert_eq!(host.realm().host_data::<crate::fetch::UrlData>(url).unwrap().params, Some(params));
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}

#[test]
fn unreferenced_url_and_query_list_cycle_is_collected() {
    let (mut env, mut host) = setup("<body></body>");
    run(&mut env, &mut host, "globalThis.savedUrl = new URL('https://example.test/?a=1'); globalThis.savedParams = savedUrl.searchParams;");
    let url = global_object(&mut host, "savedUrl");
    let params = global_object(&mut host, "savedParams");
    run(&mut env, &mut host, "delete globalThis.savedUrl; delete globalThis.savedParams; void 0;");
    host.gc();
    assert!(host.realm().heap.try_get(url).is_none(), "unreachable URL must not be permanently rooted");
    assert!(host.realm().heap.try_get(params).is_none(), "unreachable query list must not be permanently rooted");
}

#[test]
fn query_relationship_is_private_and_works_on_frozen_objects() {
    let (mut env, mut host) = setup("<body></body>");
    assert_eq!(run(&mut env, &mut host, r#"
        globalThis.savedUrl = Object.freeze(new URL('https://example.test/?a=1'));
        globalThis.savedParams = Object.freeze(savedUrl.searchParams);
        console.log(Object.getOwnPropertyNames(savedUrl).length, Object.getOwnPropertySymbols(savedUrl).length);
        console.log(Object.getOwnPropertyNames(savedParams).length, Object.getOwnPropertySymbols(savedParams).length);
        globalThis.detached = new URLSearchParams(savedParams);
        detached.set('a','detached');
        console.log(savedUrl.search, detached.get('a'));
    "#), ["0 0", "0 0", "?a=1 detached"]);
    host.gc();
    assert_eq!(run(&mut env, &mut host, "savedUrl.search='?a=changed'; console.log(savedParams.get('a'), detached.get('a'), Object.isFrozen(savedUrl), Object.isFrozen(savedParams));"), ["changed detached true true"]);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}
