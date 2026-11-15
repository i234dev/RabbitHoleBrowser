# MCP fragment navigation

GET navigation to the same resource with a changed fragment (or an explicit
fragment, including an empty one) keeps the current document. `click`, `type`
with Enter and `open` use the same Browser navigation path. The explicit
`open url=reload` path and POST requests still load a new document. A different
path/query/origin or a new tab also takes the document-loading path.

The existing Engine fragment target lookup supplies scrolling (decoded ID,
legacy anchor name, empty/top and clamping). The native ScriptHost operation
updates the existing realm's location and queues a trusted HashChangeEvent
with oldURL/newURL. Assigning location from JavaScript is not used to implement
the action, and repeating the same fragment does not generate another event.
The Page/DOM/realm/child contexts survive; stable element refs are retained.

Click/keyboard responses report `→ same-document navigation to ...` and the
normal settled action diff, not a new-page summary. `open` retains its usual
summary/snapshot options. Navigation caused by a hashchange listener is followed
with a bounded chain; an unconsumed request remains pending at the limit.

MCP history entries retain a weak document identity. Back/forward reuses the
live document only when that identity still matches, and queues popstate before
hashchange. An old entry from a replaced document cannot be mistaken for the
current document just because their URLs match. Existing script pushState,
history.go/back/forward integration, BFCache, manual scroll restoration and
joint child-frame session history are not implemented by this change. Child
fragment actions preserve that child's document and leave the top URL intact;
full named-target navigation remains the existing engine's responsibility.

This helper is initially wired into MCP, not a GUI navigation refactor.
Reference: HTML [fragment navigation](https://html.spec.whatwg.org/multipage/browsing-the-web.html#navigate-fragid)
and [scrolling to a fragment](https://html.spec.whatwg.org/multipage/browsing-the-web.html#scroll-to-fragid),
inspected 2026-11-15.

Targeted test (written, not executed during candidate preparation):

```sh
cargo test --profile ci --features jit --locked -p rabbithole-agent --test agent fragment_navigation::
```

Run with off+GCinc and on/threshold0+GCinc. The seven loopback tests check document
identity and JS memory, refs, exact event count/type/trust/URLs/order, DOM diff,
anchor scroll, GET counts, explicit reload, POST, cross-document traversal,
resource/origin changes, new-tab isolation, a child frame and hashchange-requested
navigation. They do not run authenticated sites or the RH005 model benchmark. JIT flags alone are not proof
of native-code entry.
