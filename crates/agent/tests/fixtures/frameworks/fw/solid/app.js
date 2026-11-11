// SolidJS app without a compiler: solid-js/html tagged templates (cloned
// <template> content walked by path), fine-grained signals, <For> keyed by
// item, <Show>, delegated events.
import { createSignal, For, Show, onMount } from 'solid-js';
import { render } from 'solid-js/web';
import html from 'solid-js/html';

function Counter() {
  const [n, setN] = createSignal(0);
  return html`<section id="counter"><button id="dec" onClick=${() => setN(n() - 1)}>-</button><span id="count">${n}</span><button id="inc" onClick=${() => setN(n() + 1)}>+</button></section>`;
}

let nextId = 1;
function item(text) {
  const [done, setDone] = createSignal(false);
  return { id: nextId++, text, done, setDone };
}

function Todos() {
  const [items, setItems] = createSignal([]);
  const [text, setText] = createSignal('');
  const add = (e) => {
    e.preventDefault();
    if (!text().trim()) return;
    setItems([...items(), item(text().trim())]);
    setText('');
  };
  return html`<section id="todos">
    <form id="todo-form" onSubmit=${add}>
      <input id="todo-input" value=${text} onInput=${(e) => setText(e.currentTarget.value)} />
      <button id="todo-add" type="submit">Add</button>
      <button id="todo-reverse" type="button" onClick=${() => setItems([...items()].reverse())}>Reverse</button>
    </form>
    <ul id="todo-list"><${For} each=${items}>${(it) => html`<li class="todo">
      <input type="checkbox" class="done" checked=${() => it.done()} onChange=${() => it.setDone(!it.done())} />
      <span class="text">${it.text}</span>
      <button type="button" class="del" onClick=${() => setItems(items().filter((x) => x !== it))}>x</button>
    </li>`}<//></ul>
    <p id="todo-left">${() => items().filter((x) => !x.done()).length} left</p>
  </section>`;
}

function Form() {
  const [name, setName] = createSignal('');
  const [upper, setUpper] = createSignal('');
  const [color, setColor] = createSignal('red');
  const [agree, setAgree] = createSignal(false);
  const [result, setResult] = createSignal('');
  const submit = (e) => { e.preventDefault(); setResult([name(), upper(), color(), agree() ? 'yes' : 'no'].join('|')); };
  return html`<section><form id="form" onSubmit=${submit}>
    <input id="name" value=${name} onInput=${(e) => setName(e.currentTarget.value)} />
    <p id="greeting">Hello, ${name}!</p>
    <input id="upper" value=${upper} onInput=${(e) => { const v = e.currentTarget.value.toUpperCase(); setUpper(v); e.currentTarget.value = v; }} />
    <select id="color" value=${color} onChange=${(e) => setColor(e.currentTarget.value)}>
      <option value="red">red</option><option value="green">green</option><option value="blue">blue</option>
    </select>
    <span id="color-out">${color}</span>
    <label><input id="agree" type="checkbox" checked=${agree} onChange=${(e) => setAgree(e.currentTarget.checked)} /> I agree</label>
    <button id="submit" type="submit" disabled=${() => !agree()}>Send</button>
    <p id="result">${result}</p>
  </form></section>`;
}

const base = location.pathname.replace(/about$/, '');
const routeOf = () => (/\/about$/.test(location.pathname) ? 'about' : 'home');
function Router() {
  const [route, setRoute] = createSignal(routeOf());
  addEventListener('popstate', () => setRoute(routeOf()));
  const go = (href) => (e) => { e.preventDefault(); history.pushState({}, '', href); setRoute(routeOf()); };
  return html`<section><nav><a id="link-home" href=${base} onClick=${go(base)}>Home</a> <a id="link-about" href=${base + 'about'} onClick=${go(base + 'about')}>About</a></nav>
    <div id="view">${() => (route() === 'about' ? 'About' : 'Home')}</div></section>`;
}

function Fetcher() {
  const [status, setStatus] = createSignal('');
  const [items, setItems] = createSignal([]);
  const load = async () => {
    setStatus('loading');
    const r = await fetch('/api/items');
    setItems(await r.json());
    setStatus('loaded');
  };
  return html`<section><button id="load" onClick=${load}>Load</button>
    <${Show} when=${status}><p id="status">${status}</p><//>
    <ul id="items"><${For} each=${items}>${(i) => html`<li class="item">${i.name}</li>`}<//></ul></section>`;
}

function Transition() {
  const [show, setShow] = createSignal(true);
  const [hidden, setHidden] = createSignal(false);
  let leaving = false;
  const toggle = () => {
    if (show() && !leaving) { leaving = true; setHidden(true); }
    else if (!show()) { setHidden(true); setShow(true); requestAnimationFrame(() => requestAnimationFrame(() => setHidden(false))); }
  };
  return html`<section><button id="toggle" onClick=${toggle}>Toggle</button>
    <${Show} when=${show}><div id="box" class=${() => 'box' + (hidden() ? ' hidden-state' : '')} onTransitionEnd=${() => { if (leaving) { leaving = false; setShow(false); } }}>box</div><//></section>`;
}

function App() {
  onMount(() => { window.__ready = true; });
  return html`<div>${Counter}${Todos}${Form}${Router}${Fetcher}${Transition}</div>`;
}

render(App, document.getElementById('root'));
