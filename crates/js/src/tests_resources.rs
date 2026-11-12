//! Iterator helpers and explicit resource management (`using`).

use crate::realm::Realm;

fn logs(src: &str) -> String {
    let mut realm = Realm::new();
    let logs = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let l2 = logs.clone();
    realm.set_console_sink(Box::new(move |_lvl, text| l2.borrow_mut().push(text)));
    if let Err(e) = realm.eval(src, "test.js") {
        panic!("threw: {}", realm.error_to_string(&e));
    }
    realm.run_jobs();
    let out = logs.borrow().join("|");
    out
}

#[test]
fn iterator_helpers() {
    assert_eq!(logs("console.log([1,2,3,4].values().map(x => x * 2).filter(x => x > 2).take(2).toArray().join())"), "4,6");
    assert_eq!(logs("console.log(Iterator.from({ next() { return { done: this.n > 2, value: this.n++ } }, n: 0 }).drop(1).toArray().join())"), "1,2");
    assert_eq!(logs("console.log([[1, 2], [3]].values().flatMap(x => x).reduce((a, b) => a + b))"), "6");
    assert_eq!(logs("console.log(Iterator.concat([1], [2, 3]).toArray().join(), [1,2,3,4,5].values().chunks(2).toArray().join('|'))"), "1,2,3 1,2|3,4|5");
    assert_eq!(logs("console.log(JSON.stringify(Iterator.zip([[1, 2], ['a', 'b', 'c']], { mode: 'longest' }).toArray()))"), "[[1,\"a\"],[2,\"b\"],[null,\"c\"]]");
    assert_eq!(logs("let it = [1,2].values().map(x => x); console.log(String(it), typeof it.next, Object.prototype.toString.call(Iterator.prototype))"), "[object Iterator Helper] function [object Iterator]");
}

#[test]
fn using_declarations() {
    let src = r#"
        const log = [];
        function res(name) { return { [Symbol.dispose]() { log.push('dispose ' + name); } }; }
        {
            using a = res('a'), b = res('b');
            log.push('body');
        }
        function f() { using c = res('c'); return 'ret'; }
        log.push(f());
        for (using d of [res('d1'), res('d2')]) log.push('iter');
        try {
            using e = { [Symbol.dispose]() { throw new Error('dispose'); } };
            throw new Error('body');
        } catch (err) {
            log.push(err.constructor.name + ':' + err.error.message + '/' + err.suppressed.message);
        }
        console.log(log.join());
    "#;
    assert_eq!(logs(src), "body,dispose b,dispose a,dispose c,ret,iter,dispose d1,iter,dispose d2,SuppressedError:dispose/body");
    let src = r#"
        const log = [];
        async function g() {
            await using x = { async [Symbol.asyncDispose]() { log.push('async dispose'); } };
            log.push('body');
        }
        g().then(() => { log.push('done'); console.log(log.join()); });
    "#;
    assert_eq!(logs(src), "body,async dispose,done");
    let src = r#"
        const s = new DisposableStack();
        const log = [];
        s.defer(() => log.push(1));
        s.adopt(2, v => log.push(v));
        s.dispose();
        console.log(log.join(), s.disposed);
        let using = 5; using += 1; console.log(using);
    "#;
    assert_eq!(logs(src), "2,1 true|6");
}
