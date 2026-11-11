// Shared React app (React 18 and 19): createElement without JSX, hooks,
// Suspense, controlled inputs, keyed lists, a pushState router and a
// transitionend-driven exit animation. Exposes window.ReactFixture.
(function () {
  'use strict';
  var R = window.React;
  var h = R.createElement, useState = R.useState, useEffect = R.useEffect, useRef = R.useRef;

  function Counter() {
    var s = useState(0), n = s[0], setN = s[1];
    return h('section', { id: 'counter' },
      h('button', { id: 'dec', onClick: function () { setN(n - 1); } }, '-'),
      h('span', { id: 'count' }, n),
      h('button', { id: 'inc', onClick: function () { setN(function (c) { return c + 1; }); } }, '+'));
  }

  function Todos() {
    var s1 = useState([]), items = s1[0], setItems = s1[1];
    var s2 = useState(''), text = s2[0], setText = s2[1];
    var next = useRef(1);
    function add(e) {
      e.preventDefault();
      if (!text.trim()) return;
      setItems(items.concat([{ id: next.current++, text: text.trim(), done: false }]));
      setText('');
    }
    return h('section', { id: 'todos' },
      h('form', { id: 'todo-form', onSubmit: add },
        h('input', { id: 'todo-input', value: text, onChange: function (e) { setText(e.target.value); } }),
        h('button', { id: 'todo-add', type: 'submit' }, 'Add'),
        h('button', { id: 'todo-reverse', type: 'button', onClick: function () { setItems(items.slice().reverse()); } }, 'Reverse')),
      h('ul', { id: 'todo-list' }, items.map(function (it) {
        return h('li', { key: it.id, className: 'todo' },
          h('input', { type: 'checkbox', className: 'done', checked: it.done, onChange: function () {
            setItems(items.map(function (x) { return x.id === it.id ? Object.assign({}, x, { done: !x.done }) : x; }));
          } }),
          h('span', { className: 'text' }, it.text),
          h('button', { type: 'button', className: 'del', onClick: function () { setItems(items.filter(function (x) { return x.id !== it.id; })); } }, 'x'));
      })),
      h('p', { id: 'todo-left' }, items.filter(function (x) { return !x.done; }).length + ' left'));
  }

  function Form() {
    var a = useState(''), name = a[0], setName = a[1];
    var b = useState(''), upper = b[0], setUpper = b[1];
    var c = useState('red'), color = c[0], setColor = c[1];
    var d = useState(false), agree = d[0], setAgree = d[1];
    var e = useState(''), result = e[0], setResult = e[1];
    return h('section', null, h('form', { id: 'form', onSubmit: function (ev) { ev.preventDefault(); setResult([name, upper, color, agree ? 'yes' : 'no'].join('|')); } },
      h('input', { id: 'name', value: name, onChange: function (ev) { setName(ev.target.value); } }),
      h('p', { id: 'greeting' }, 'Hello, ' + name + '!'),
      h('input', { id: 'upper', value: upper, onChange: function (ev) { setUpper(ev.target.value.toUpperCase()); } }),
      h('select', { id: 'color', value: color, onChange: function (ev) { setColor(ev.target.value); } },
        ['red', 'green', 'blue'].map(function (o) { return h('option', { key: o, value: o }, o); })),
      h('span', { id: 'color-out' }, color),
      h('label', null, h('input', { id: 'agree', type: 'checkbox', checked: agree, onChange: function (ev) { setAgree(ev.target.checked); } }), ' I agree'),
      h('button', { id: 'submit', type: 'submit', disabled: !agree }, 'Send'),
      h('p', { id: 'result' }, result)));
  }

  var base = location.pathname.replace(/about$/, '');
  function routeOf() { return /\/about$/.test(location.pathname) ? 'about' : 'home'; }
  function Router() {
    var s = useState(routeOf()), route = s[0], setRoute = s[1];
    useEffect(function () {
      function pop() { setRoute(routeOf()); }
      window.addEventListener('popstate', pop);
      return function () { window.removeEventListener('popstate', pop); };
    }, []);
    function link(id, href, label) {
      return h('a', { id: id, href: href, onClick: function (e) { e.preventDefault(); history.pushState({}, '', href); setRoute(routeOf()); } }, label);
    }
    return h('section', null, h('nav', null, link('link-home', base, 'Home'), ' ', link('link-about', base + 'about', 'About')),
      h('div', { id: 'view' }, route === 'about' ? 'About' : 'Home'));
  }

  function resource(promise) {
    var state = 'pending', value;
    var p = promise.then(function (v) { state = 'done'; value = v; }, function (e) { state = 'error'; value = e; });
    return { read: function () { if (state === 'pending') throw p; if (state === 'error') throw value; return value; } };
  }
  function Items(props) {
    var data = props.res.read();
    return h(R.Fragment, null,
      h('ul', { id: 'items' }, data.map(function (i) { return h('li', { key: i.id, className: 'item' }, i.name); })),
      h('p', { id: 'status' }, 'loaded'));
  }
  function Fetcher() {
    var s = useState(null), res = s[0], setRes = s[1];
    return h('section', null,
      h('button', { id: 'load', onClick: function () { setRes(resource(fetch('/api/items').then(function (r) { return r.json(); }))); } }, 'Load'),
      res && h(R.Suspense, { fallback: h('p', { id: 'status' }, 'loading') }, h(Items, { res: res })));
  }

  function Transition() {
    var a = useState(true), show = a[0], setShow = a[1];
    var b = useState(false), hidden = b[0], setHidden = b[1];
    var leaving = useRef(false);
    function toggle() {
      if (show && !leaving.current) { leaving.current = true; setHidden(true); }
      else if (!show) {
        setShow(true); setHidden(true);
        requestAnimationFrame(function () { requestAnimationFrame(function () { setHidden(false); }); });
      }
    }
    return h('section', null, h('button', { id: 'toggle', onClick: toggle }, 'Toggle'),
      show && h('div', { id: 'box', className: 'box' + (hidden ? ' hidden-state' : ''), onTransitionEnd: function () {
        if (leaving.current) { leaving.current = false; setShow(false); }
      } }, 'box'));
  }

  function App() {
    useEffect(function () { window.__ready = true; }, []);
    return h('div', null, h(Counter), h(Todos), h(Form), h(Router), h(Fetcher), h(Transition));
  }

  // Server-rendered page: counter, a conditional block and a list.
  function SsrApp() {
    var a = useState(0), n = a[0], setN = a[1];
    var b = useState(true), cond = b[0], setCond = b[1];
    useEffect(function () { window.__ready = true; }, []);
    return h('div', { id: 'ssr' },
      h('button', { id: 'inc', onClick: function () { setN(n + 1); } }, '+'),
      h('span', { id: 'count' }, n),
      h('button', { id: 'cond-toggle', onClick: function () { setCond(!cond); } }, 'toggle'),
      cond ? h('p', { id: 'cond' }, 'shown') : null,
      n > 0 ? h('p', { id: 'positive' }, 'positive') : null,
      h('ul', null, ['one', 'two', 'three'].map(function (t) { return h('li', { key: t, className: 'ssr-item' }, t); })));
  }

  window.ReactFixture = { App: App, SsrApp: SsrApp };
})();
