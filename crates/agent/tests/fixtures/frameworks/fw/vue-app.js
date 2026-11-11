// Vue 3 app with in-DOM / string templates compiled at runtime.
(function () {
  'use strict';
  var V = window.Vue;
  var base = location.pathname.replace(/about$/, '');
  function routeOf() { return /\/about$/.test(location.pathname) ? 'about' : 'home'; }

  var Counter = {
    setup: function () { return { n: V.ref(0) }; },
    template: '<section id="counter"><button id="dec" @click="n--">-</button><span id="count">{{ n }}</span><button id="inc" @click="n++">+</button></section>'
  };

  var Todos = {
    setup: function () {
      var items = V.ref([]), text = V.ref(''), next = 1;
      return {
        items: items, text: text,
        left: V.computed(function () { return items.value.filter(function (i) { return !i.done; }).length; }),
        add: function () { if (!text.value.trim()) return; items.value.push({ id: next++, text: text.value.trim(), done: false }); text.value = ''; },
        reverse: function () { items.value.reverse(); },
        del: function (it) { items.value = items.value.filter(function (x) { return x !== it; }); }
      };
    },
    template: '<section id="todos"><form id="todo-form" @submit.prevent="add">' +
      '<input id="todo-input" v-model="text"><button id="todo-add" type="submit">Add</button>' +
      '<button id="todo-reverse" type="button" @click="reverse">Reverse</button></form>' +
      '<ul id="todo-list"><li v-for="it in items" :key="it.id" class="todo">' +
      '<input type="checkbox" class="done" v-model="it.done"><span class="text">{{ it.text }}</span>' +
      '<button type="button" class="del" @click="del(it)">x</button></li></ul>' +
      '<p id="todo-left">{{ left }} left</p></section>'
  };

  var Form = {
    setup: function () {
      var s = V.reactive({ name: '', upper: '', color: 'red', agree: false, result: '' });
      return {
        s: s,
        setUpper: function (e) { s.upper = e.target.value.toUpperCase(); e.target.value = s.upper; },
        submit: function () { s.result = [s.name, s.upper, s.color, s.agree ? 'yes' : 'no'].join('|'); }
      };
    },
    template: '<section><form id="form" @submit.prevent="submit">' +
      '<input id="name" v-model="s.name"><p id="greeting">Hello, {{ s.name }}!</p>' +
      '<input id="upper" :value="s.upper" @input="setUpper">' +
      '<select id="color" v-model="s.color"><option v-for="c in [\'red\',\'green\',\'blue\']" :value="c">{{ c }}</option></select>' +
      '<span id="color-out">{{ s.color }}</span>' +
      '<label><input id="agree" type="checkbox" v-model="s.agree"> I agree</label>' +
      '<button id="submit" type="submit" :disabled="!s.agree">Send</button><p id="result">{{ s.result }}</p></form></section>'
  };

  var Router = {
    setup: function () {
      var route = V.ref(routeOf());
      function pop() { route.value = routeOf(); }
      V.onMounted(function () { window.addEventListener('popstate', pop); });
      V.onUnmounted(function () { window.removeEventListener('popstate', pop); });
      return { route: route, base: base, go: function (href) { history.pushState({}, '', href); route.value = routeOf(); } };
    },
    template: '<section><nav><a id="link-home" :href="base" @click.prevent="go(base)">Home</a> ' +
      '<a id="link-about" :href="base + \'about\'" @click.prevent="go(base + \'about\')">About</a></nav>' +
      '<div id="view">{{ route === "about" ? "About" : "Home" }}</div></section>'
  };

  var Fetcher = {
    setup: function () {
      var status = V.ref(''), items = V.ref([]);
      return { status: status, items: items, load: function () {
        status.value = 'loading';
        fetch('/api/items').then(function (r) { return r.json(); }).then(function (d) { items.value = d; status.value = 'loaded'; });
      } };
    },
    template: '<section><button id="load" @click="load">Load</button><p id="status" v-if="status">{{ status }}</p>' +
      '<ul id="items"><li v-for="i in items" :key="i.id" class="item">{{ i.name }}</li></ul></section>'
  };

  var Trans = {
    setup: function () { return { show: V.ref(true) }; },
    template: '<section><button id="toggle" @click="show = !show">Toggle</button>' +
      '<Transition name="fade"><div id="box" v-if="show">box</div></Transition></section>'
  };

  var App = {
    components: { Counter: Counter, Todos: Todos, VForm: Form, Router: Router, Fetcher: Fetcher, Trans: Trans },
    mounted: function () { window.__ready = true; },
    template: '<div><Counter/><Todos/><VForm/><Router/><Fetcher/><Trans/></div>'
  };

  var SsrApp = {
    setup: function () { return { n: V.ref(0), cond: V.ref(true), items: ['one', 'two', 'three'] }; },
    mounted: function () { window.__ready = true; },
    template: '<div id="ssr"><button id="inc" @click="n++">+</button><span id="count">{{ n }}</span>' +
      '<button id="cond-toggle" @click="cond = !cond">toggle</button>' +
      '<p id="cond" v-if="cond">shown</p><p id="positive" v-if="n > 0">positive</p>' +
      '<ul><li v-for="t in items" :key="t" class="ssr-item">{{ t }}</li></ul></div>'
  };

  window.VueFixture = { App: App, SsrApp: SsrApp };
})();
