// Lit 3 app: custom elements with shadow DOM, static styles (adopted
// stylesheets), reactive properties, repeat() keyed lists, live() inputs.
import { LitElement, html, css, repeat, live } from '/vendor/lit/lit-all.min.js';

const shared = css`
  :host { display: block; border: 1px solid #ccc; margin: 6px 0; padding: 6px; }
  button, input, select { font: inherit; margin: 2px; }
  .box { width: 160px; height: 40px; background: teal; color: white; opacity: 1; transition: opacity 0.3s linear; }
  .box.hidden-state { opacity: 0; }
`;

class FxCounter extends LitElement {
  static properties = { n: { state: true } };
  static styles = [shared, css`#count { font-weight: bold; color: rgb(0, 0, 255); }`];
  constructor() { super(); this.n = 0; }
  render() {
    return html`<button id="dec" @click=${() => this.n--}>-</button><span id="count">${this.n}</span><button id="inc" @click=${() => this.n++}>+</button>`;
  }
}
customElements.define('fx-counter', FxCounter);

class FxTodos extends LitElement {
  static properties = { items: { state: true }, text: { state: true } };
  static styles = shared;
  constructor() { super(); this.items = []; this.text = ''; this.next = 1; }
  add(e) {
    e.preventDefault();
    if (!this.text.trim()) return;
    this.items = [...this.items, { id: this.next++, text: this.text.trim(), done: false }];
    this.text = '';
  }
  render() {
    return html`<form id="todo-form" @submit=${this.add}>
      <input id="todo-input" .value=${live(this.text)} @input=${(e) => (this.text = e.target.value)}>
      <button id="todo-add" type="submit">Add</button>
      <button id="todo-reverse" type="button" @click=${() => (this.items = [...this.items].reverse())}>Reverse</button>
    </form>
    <ul id="todo-list">${repeat(this.items, (it) => it.id, (it) => html`<li class="todo">
      <input type="checkbox" class="done" .checked=${it.done} @change=${() => (this.items = this.items.map((x) => (x === it ? { ...x, done: !x.done } : x)))}>
      <span class="text">${it.text}</span>
      <button type="button" class="del" @click=${() => (this.items = this.items.filter((x) => x !== it))}>x</button></li>`)}</ul>
    <p id="todo-left">${this.items.filter((x) => !x.done).length} left</p>`;
  }
}
customElements.define('fx-todos', FxTodos);

class FxForm extends LitElement {
  static properties = { name: { state: true }, upper: { state: true }, color: { state: true }, agree: { state: true }, result: { state: true } };
  static styles = shared;
  constructor() { super(); this.name = ''; this.upper = ''; this.color = 'red'; this.agree = false; this.result = ''; }
  render() {
    return html`<form id="form" @submit=${(e) => { e.preventDefault(); this.result = [this.name, this.upper, this.color, this.agree ? 'yes' : 'no'].join('|'); }}>
      <input id="name" .value=${live(this.name)} @input=${(e) => (this.name = e.target.value)}>
      <p id="greeting">Hello, ${this.name}!</p>
      <input id="upper" .value=${live(this.upper)} @input=${(e) => (this.upper = e.target.value.toUpperCase())}>
      <select id="color" .value=${this.color} @change=${(e) => (this.color = e.target.value)}>
        ${['red', 'green', 'blue'].map((c) => html`<option value=${c} ?selected=${c === this.color}>${c}</option>`)}
      </select>
      <span id="color-out">${this.color}</span>
      <label><input id="agree" type="checkbox" .checked=${this.agree} @change=${(e) => (this.agree = e.target.checked)}> I agree</label>
      <button id="submit" type="submit" ?disabled=${!this.agree}>Send</button>
      <p id="result">${this.result}</p></form>`;
  }
}
customElements.define('fx-form', FxForm);

const base = location.pathname.replace(/about$/, '');
const routeOf = () => (/\/about$/.test(location.pathname) ? 'about' : 'home');
class FxRouter extends LitElement {
  static properties = { route: { state: true } };
  static styles = shared;
  constructor() { super(); this.route = routeOf(); this.pop = () => (this.route = routeOf()); }
  connectedCallback() { super.connectedCallback(); addEventListener('popstate', this.pop); }
  disconnectedCallback() { super.disconnectedCallback(); removeEventListener('popstate', this.pop); }
  go(e) { e.preventDefault(); history.pushState({}, '', e.currentTarget.getAttribute('href')); this.route = routeOf(); }
  render() {
    return html`<nav><a id="link-home" href=${base} @click=${this.go}>Home</a> <a id="link-about" href=${base + 'about'} @click=${this.go}>About</a></nav>
      <div id="view">${this.route === 'about' ? 'About' : 'Home'}</div>`;
  }
}
customElements.define('fx-router', FxRouter);

class FxFetch extends LitElement {
  static properties = { status: { state: true }, items: { state: true } };
  static styles = shared;
  constructor() { super(); this.status = ''; this.items = []; }
  async load() {
    this.status = 'loading';
    this.items = await (await fetch('/api/items')).json();
    this.status = 'loaded';
  }
  render() {
    return html`<button id="load" @click=${this.load}>Load</button>${this.status ? html`<p id="status">${this.status}</p>` : ''}
      <ul id="items">${repeat(this.items, (i) => i.id, (i) => html`<li class="item">${i.name}</li>`)}</ul>`;
  }
}
customElements.define('fx-fetch', FxFetch);

class FxTransition extends LitElement {
  static properties = { show: { state: true }, hidden: { state: true } };
  static styles = shared;
  constructor() { super(); this.show = true; this.hidden = false; this.leaving = false; }
  toggle() {
    if (this.show && !this.leaving) { this.leaving = true; this.hidden = true; }
    else if (!this.show) { this.show = true; this.hidden = true; requestAnimationFrame(() => requestAnimationFrame(() => (this.hidden = false))); }
  }
  render() {
    return html`<button id="toggle" @click=${this.toggle}>Toggle</button>
      ${this.show ? html`<div id="box" class="box ${this.hidden ? 'hidden-state' : ''}" @transitionend=${() => { if (this.leaving) { this.leaving = false; this.show = false; } }}>box</div>` : ''}`;
  }
}
customElements.define('fx-transition', FxTransition);

// Declaratively rendered on the server (shadowrootmode template): the
// element upgrades onto the existing shadow root and keeps its nodes.
class FxSsr extends LitElement {
  static properties = { n: { state: true }, cond: { state: true } };
  constructor() { super(); this.n = 0; this.cond = true; }
  createRenderRoot() {
    // Reuse the declarative shadow root: wire the server-rendered nodes
    // instead of rendering a new template over them.
    const root = this.shadowRoot || this.attachShadow({ mode: 'open' });
    this.ssrRoot = root;
    return root;
  }
  firstUpdated() { window.__ready = true; }
  update(changed) {
    // Lit's client renderer would replace the server DOM; drive it by hand.
    super.update(new Map());
    const r = this.ssrRoot;
    r.getElementById('count').textContent = String(this.n);
    const cond = r.getElementById('cond');
    if (this.cond && !cond) { const p = document.createElement('p'); p.id = 'cond'; p.textContent = 'shown'; r.getElementById('cond-toggle').after(p); }
    if (!this.cond && cond) cond.remove();
    if (this.n > 0 && !r.getElementById('positive')) { const p = document.createElement('p'); p.id = 'positive'; p.textContent = 'positive'; r.querySelector('ul').before(p); }
  }
  render() { return html``; }
  connectedCallback() {
    super.connectedCallback();
    const r = this.shadowRoot;
    r.getElementById('inc').addEventListener('click', () => this.n++);
    r.getElementById('cond-toggle').addEventListener('click', () => (this.cond = !this.cond));
  }
}

if (document.body.dataset.ssr) {
  const host = document.querySelector('fx-ssr');
  const r = host.shadowRoot;
  window.__ssr = { count: r && r.getElementById('count'), inc: r && r.getElementById('inc') };
  customElements.define('fx-ssr', FxSsr);
} else {
  customElements.whenDefined('fx-transition').then(() => requestAnimationFrame(() => { window.__ready = true; }));
}
