// Preact app (ES modules through an import map): hooks, keyed lists,
// controlled inputs (onInput), pushState router, transitionend exit.
import { h, render, hydrate, Fragment } from 'preact';
import { useState, useEffect, useRef } from 'preact/hooks';

function Counter() {
  const [n, setN] = useState(0);
  return h('section', { id: 'counter' },
    h('button', { id: 'dec', onClick: () => setN(n - 1) }, '-'),
    h('span', { id: 'count' }, n),
    h('button', { id: 'inc', onClick: () => setN((c) => c + 1) }, '+'));
}

function Todos() {
  const [items, setItems] = useState([]);
  const [text, setText] = useState('');
  const next = useRef(1);
  const add = (e) => {
    e.preventDefault();
    if (!text.trim()) return;
    setItems([...items, { id: next.current++, text: text.trim(), done: false }]);
    setText('');
  };
  return h('section', { id: 'todos' },
    h('form', { id: 'todo-form', onSubmit: add },
      h('input', { id: 'todo-input', value: text, onInput: (e) => setText(e.currentTarget.value) }),
      h('button', { id: 'todo-add', type: 'submit' }, 'Add'),
      h('button', { id: 'todo-reverse', type: 'button', onClick: () => setItems(items.slice().reverse()) }, 'Reverse')),
    h('ul', { id: 'todo-list' }, items.map((it) => h('li', { key: it.id, class: 'todo' },
      h('input', { type: 'checkbox', class: 'done', checked: it.done, onChange: () => setItems(items.map((x) => (x.id === it.id ? { ...x, done: !x.done } : x))) }),
      h('span', { class: 'text' }, it.text),
      h('button', { type: 'button', class: 'del', onClick: () => setItems(items.filter((x) => x.id !== it.id)) }, 'x')))),
    h('p', { id: 'todo-left' }, `${items.filter((x) => !x.done).length} left`));
}

function Form() {
  const [name, setName] = useState('');
  const [upper, setUpper] = useState('');
  const [color, setColor] = useState('red');
  const [agree, setAgree] = useState(false);
  const [result, setResult] = useState('');
  return h('section', null, h('form', { id: 'form', onSubmit: (e) => { e.preventDefault(); setResult([name, upper, color, agree ? 'yes' : 'no'].join('|')); } },
    h('input', { id: 'name', value: name, onInput: (e) => setName(e.currentTarget.value) }),
    h('p', { id: 'greeting' }, `Hello, ${name}!`),
    h('input', { id: 'upper', value: upper, onInput: (e) => { const v = e.currentTarget.value.toUpperCase(); setUpper(v); e.currentTarget.value = v; } }),
    h('select', { id: 'color', value: color, onChange: (e) => setColor(e.currentTarget.value) }, ['red', 'green', 'blue'].map((o) => h('option', { value: o }, o))),
    h('span', { id: 'color-out' }, color),
    h('label', null, h('input', { id: 'agree', type: 'checkbox', checked: agree, onChange: (e) => setAgree(e.currentTarget.checked) }), ' I agree'),
    h('button', { id: 'submit', type: 'submit', disabled: !agree }, 'Send'),
    h('p', { id: 'result' }, result)));
}

const base = location.pathname.replace(/about$/, '');
const routeOf = () => (/\/about$/.test(location.pathname) ? 'about' : 'home');
function Router() {
  const [route, setRoute] = useState(routeOf());
  useEffect(() => {
    const pop = () => setRoute(routeOf());
    addEventListener('popstate', pop);
    return () => removeEventListener('popstate', pop);
  }, []);
  const link = (id, href, label) => h('a', { id, href, onClick: (e) => { e.preventDefault(); history.pushState({}, '', href); setRoute(routeOf()); } }, label);
  return h('section', null, h('nav', null, link('link-home', base, 'Home'), ' ', link('link-about', `${base}about`, 'About')),
    h('div', { id: 'view' }, route === 'about' ? 'About' : 'Home'));
}

function Fetcher() {
  const [status, setStatus] = useState('');
  const [items, setItems] = useState([]);
  const load = async () => {
    setStatus('loading');
    const r = await fetch('/api/items');
    setItems(await r.json());
    setStatus('loaded');
  };
  return h('section', null, h('button', { id: 'load', onClick: load }, 'Load'),
    status && h('p', { id: 'status' }, status),
    h('ul', { id: 'items' }, items.map((i) => h('li', { key: i.id, class: 'item' }, i.name))));
}

function Transition() {
  const [show, setShow] = useState(true);
  const [hidden, setHidden] = useState(false);
  const leaving = useRef(false);
  const toggle = () => {
    if (show && !leaving.current) { leaving.current = true; setHidden(true); }
    else if (!show) { setShow(true); setHidden(true); requestAnimationFrame(() => requestAnimationFrame(() => setHidden(false))); }
  };
  return h('section', null, h('button', { id: 'toggle', onClick: toggle }, 'Toggle'),
    show && h('div', { id: 'box', class: `box${hidden ? ' hidden-state' : ''}`, onTransitionEnd: () => { if (leaving.current) { leaving.current = false; setShow(false); } } }, 'box'));
}

function App() {
  useEffect(() => { window.__ready = true; }, []);
  return h(Fragment, null, h(Counter), h(Todos), h(Form), h(Router), h(Fetcher), h(Transition));
}

function SsrApp() {
  const [n, setN] = useState(0);
  const [cond, setCond] = useState(true);
  useEffect(() => { window.__ready = true; }, []);
  return h('div', { id: 'ssr' },
    h('button', { id: 'inc', onClick: () => setN(n + 1) }, '+'),
    h('span', { id: 'count' }, n),
    h('button', { id: 'cond-toggle', onClick: () => setCond(!cond) }, 'toggle'),
    cond ? h('p', { id: 'cond' }, 'shown') : null,
    n > 0 ? h('p', { id: 'positive' }, 'positive') : null,
    h('ul', null, ['one', 'two', 'three'].map((t) => h('li', { key: t, class: 'ssr-item' }, t))));
}

const root = document.getElementById('root');
if (document.body.dataset.ssr) hydrate(h(SsrApp), root);
else render(h(App), root);
