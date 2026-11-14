# Opt-in WPT mouse actions

`wpt_reftest --mouse-actions` enables a small testdriver backend for WPT
reftests. The default runner and ordinary browser pages retain their existing
behavior. This is not a WebDriver implementation or a product web API.

The WPT HTTP server serves an embedded vendor backend only at
`/resources/testdriver-vendor.js`, and only in the opt-in mode. It requires the
corpus file to exist and be empty; an existing nonempty vendor causes an explicit
setup error. No corpus file is rewritten. Both the run header and summary print
the automation mode; keep that mode with source/runner hashes when recording
results. Raw and opt-in results must not be presented as the same profile.

Example (run only when the coordinator schedules the build/runtime):

```sh
wpt_reftest --wpt /path/to/wpt --mouse-actions --jobs 1 --mem-kb 1200000 \
  --filter remove-hovered-element css/selectors
```

## Supported input

The backend accepts one persistent mouse source ID and one `none` source per
sequence. The mouse position and primary-button state persist across ordered
`send()` calls. Concurrent sends queue in call order. Repeated down/up in the
same state produces no extra event. `none` pauses and pointer pauses share a
tick; its wait is their maximum. The stock Actions builder's default `none`
source and its 16ms pauses are supported.

Supported mouse operations are instantaneous moves from viewport, current
pointer position, or an Element in the current document; primary down/up; and
pauses. Element coordinates use the current first client rectangle's clipped
in-view center, checked at execution time. No automatic scroll or move-away is
inserted. The backend dispatches `Engine::pointer_move/down/up`, including real
hit testing, hover/active/focus, trusted pointer/mouse events and activation.
Label activation applies the returned focus action. Link/form navigation is
explicitly unsupported and cancels remaining actions.

Other pointer types, buttons, movement interpolation, keyboard/wheel input,
non-default pressure/tilt fields, multiple mouse IDs and other contexts reject
before the sequence is published. This first subset excludes pages containing
child frames, including frames created during a sequence. Unsupported automation
is a harness ERROR even if page code catches the Promise rejection or clears
`reftest-wait`; it never becomes a success-shaped no-op.

## Lifetime and execution boundary

The vendor normalizes the whole sequence before queueing it, retains Elements
and Promise callbacks in its JS closure, and publishes primitive steps only.
Rust never holds raw JS object IDs. Each poll drains earlier microtasks before
reading current geometry, then returns from JS before dispatching Engine input.
After the tick's pause, a bounded acknowledgement resolves the sequence and
runs its Promise checkpoint. The runner continues until both the screenshot
wait and the action queue are empty, then observes rendering/timer/image work
under the same snapshot deadline.

Queue ownership is bound to the current Page/frame and captured document root.
Navigation, new windows, dialogs, root replacement, detached origins and frame
contexts produce cancellation. Ordinary DOM mutations do not invalidate the
whole queue: removing a hovered element is the behavior under test. Tick,
checkpoint and input outcomes are all observed; the opt-in pump does not use
`settle_for_snapshot`, which discards tick navigation outcomes.

Limits: 4 queued sequences, 128 ticks per sequence, 256 queued ticks, 1000ms
per pause, 4000ms total pause per sequence, 65535 request IDs and 1024 executed
ticks per render. The existing harness deadline and worker memory/time limits
still apply. When the realm is still runnable, failure rejects queued Promises; timeout or
watchdog failures discard the page without extending its budget. No
cleanup click, release or hover reset can hide a browser bug.

## Budgets and remaining limitations

Poll/ack use bounded script checkpoints rather than the developer console's
10-second evaluation helper. Event/timer callbacks retain normal engine
watchdogs; aborts remain ERRORs. No global Realm deadline ceiling is introduced:
callbacks may start separate normal task budgets, and existing watchdog credit
for synchronous native work remains. Outer worker timeouts remain authoritative
for a stuck process. This does not claim one hard absolute JS deadline spanning
every callback in a pointer action.

Existing pointer-event limitations remain browser issues. In particular, the
engine's current `MouseEvent.buttons` construction does not model every move/up
button state correctly. The bridge does not rewrite those fields or treat its
limited successful tests as pointer/WebDriver conformance.

Primary action semantics: [WebDriver actions](https://w3c.github.io/webdriver/#actions)
and [in-view center](https://w3c.github.io/webdriver/#dfn-in-view-center-point).
