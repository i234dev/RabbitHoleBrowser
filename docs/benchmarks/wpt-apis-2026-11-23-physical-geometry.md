# Web API validation at 11:55 CET on 23 November 2026

The last complete RabbitHole comparison on the fixed 40,172 browser URLs passed 1,338,622/1,773,649 subtests (75.47%) at `d9db0456`. Main at `a43c8e0` passed 781,820/1,773,653 (44.08%). At 11:50 the full intermediate run at `367874b4` had covered 33,507 matched URLs across 201 directories. On those URLs main passed 641,010/1,506,786 and the intermediate candidate passed 1,242,876/1,509,207, with 15,646 previous named passes missing or failing. A separate full run started at 11:55 with the frozen executable for `32d18bea`. [Last complete comparison by directory](wpt-apis-2026-11-23-directories.md)

## Focused Web Platform Tests

The following counts compare named subtests on matched URLs against `d9db0456`. Duplicate names retain their multiplicity. TheOtherSide used the local WPT checkout at `831baf28d226911e47b2454d69e0c9e7348dd4d1`, the Cargo ci profile, a loopback-only network namespace, ten runner workers on CPUs 0–3 and 10–15 and 5-second or 15-second deadlines. Different totals reflect which subtests reached the harness.

| Directory or selection | URLs | Earlier passes / total | Later passes / total | Gained | Lost |
|---|---:|---:|---:|---:|---:|
| css/cssom | 272 | 2,812 / 4,458 | 3,555 / 4,458 | 743 | 0 |
| css/css-grid/animation and css/css-grid/grid-lanes/animation | 16 | 1,692 / 3,784 | 3,752 / 3,784 | 2,060 | 0 |
| editing | 727 | 65,957 / 115,080 | 105,658 / 116,234 | 39,937 | 236 |
| webrtc | 265 | 26 / 2,339 | 805 / 2,340 | 779 | 0 |
| html/semantics/popovers | 94 | 1,683 / 3,920 | 3,467 / 3,921 | 1,805 | 21 |
| WebCryptoAPI, NIST signature and agreement selection | 8 | 0 / 2,994 | 2,928 / 3,072 | 2,928 | 0 |

The grid, popover and WebCrypto binary included `0da89c6a` (SHA-256 `66c4ad14e18465e968e589a8ae5d12fe488842c27cdc1ab6a6a81ea3b84e929f`). WebRTC included `78497c88` and the document referrer changes subsequently committed as `fb95a8cf` (SHA-256 `68c3a3fa3deb29c35887265df25ed50ac50ec345d2c056ff8265a871d30cb6ce`). Editing included the list commands at `dd1d9de5` and those same referrer changes. CSSOM and the full run use `32d18bea` (SHA-256 `dabbbb18ad34578165f7060d9ffd99c63c8e8a59e48e0b6b33e50cd993ac5b65`). The source manifest for the frozen full run matched the revision for all tracked Rust, JS and TOML sources and Cargo.lock.

## Changes, controls and remaining failures

- **CSSOM and animation.** `faea391d` resolves namespaces when parsing selectors and assigning selectorText. `4a386f05` composites keyframes in animation order. `32d18bea` maps layout coordinates through orthogonal writing modes before computing physical CSSOM dimensions. This last change gained 416 named CSSOM passes over the previous run. Geometry changes affect script measurements and hit regions. The integration test checks nested orthogonal boxes, their client dimensions and resolved percentage insets. The animation group passed all 12 tests.
- **Rich editing.** `16e98f71` preserves caret lines and styles on paragraph breaks, `0da89c6a` removes formatting from partial selections, and `dd1d9de5` creates, converts and toggles lists while preserving a backward selection. The first two changes reached 104,641/116,174 before the list change. The subsequent comparison loses 19 previous named passes, including mixed nested-list state and repeated edits in table cells. Across all 727 pages the current named pass rate is 90.9%. The rich editing and list integration group passed 46/46. Selections across tables, white-space normalization and nested block restructuring still need work.
- **WebRTC.** `74cf22e2` exposes ICE, DTLS and SCTP state and negotiated message limits. `78497c88` publishes the selected pair before the connected notification. Two IDL setup pages had timed out while waiting for this state. Both completed after the change. The real two-peer test checks selected endpoints, negotiated oversize rejection, delivery and remote close. RTP media, relay connectivity and devices require separate validation.
- **HTML popovers.** `a486eab9` unifies script activation, CSS matching, painting and hit testing. All three top-layer integration tests passed. Of the 21 named losses, 20 cover outside pointer dismissal and one covers a nested opening during a closing beforetoggle handler. Pointer ordering and reentrant top-layer operations need separate controls.
- **Fetch and cryptography.** `fb95a8cf` applies the effective document referrer policy to script requests and recognises trustworthy loopback destinations. A loopback HTTP server checked inherited suppression on fetch and XHR and the explicit request override. All 19 service-worker integration tests and 71 network library tests passed. `45cdce9e` signs and verifies ECDSA for three NIST curves independently of the hash. All 271 script library tests passed, including a known-signature vector and invalid signatures. Many referrer WPT pages need Python server fixtures that the native static fixture server cannot serve.
- **Cross-realm DOM.** The two Range insertion and surround pages still pass 0/3,680. They exercise live child-realm ranges through a same-origin window. The XML and XHTML child-document setup also fails on the namespace creation page (204/596). These failures share setup and object access paths.

The physical-geometry integration group passed 47/47. The latest full normal and forced-JIT test262 comparison was at `452f5495`, with 46,017/46,064 and zero regressions in each mode. Final full-branch test262, GC stress, eight-pair Speedometer and named API regression sign-off remain pending. [Earlier independent controls](wpt-apis-2026-11-23-grid-focus.md)

## Complete CSS reftests

TheOtherSide ran the frozen CSS executable with SHA-256 `03b627a75621c5c666b53b33ab1f47c2e4ebfaebd32a64cf650605da91a0d484` on the local reftest checkout, with ten workers, the ci profile and a 20-second timeout. It includes the popover, namespace, keyframe and ICE changes through `78497c88` plus the document referrer changes. It predates the list and physical-geometry changes. All 19,627 paths matched main. The comparison found 29 gains and zero losses.

| Directory | Pages | Main passed | Candidate passed | Lost |
|---|---:|---:|---:|---:|
| css/CSS2 | 6,232 | 5,782 | 5,783 | 0 |
| css/compositing | 60 | 36 | 36 | 0 |
| css/css-align | 64 | 37 | 37 | 0 |
| css/css-backgrounds | 718 | 607 | 607 | 0 |
| css/css-borders | 123 | 42 | 42 | 0 |
| css/css-box | 42 | 38 | 38 | 0 |
| css/css-cascade | 51 | 41 | 41 | 0 |
| css/css-color | 311 | 291 | 295 | 0 |
| css/css-conditional | 172 | 149 | 150 | 0 |
| css/css-contain | 435 | 307 | 307 | 0 |
| css/css-display | 223 | 179 | 179 | 0 |
| css/css-flexbox | 1,027 | 875 | 879 | 0 |
| css/css-fonts | 387 | 262 | 262 | 0 |
| css/css-grid | 1,763 | 1,390 | 1,396 | 0 |
| css/css-images | 482 | 434 | 434 | 0 |
| css/css-masking | 384 | 268 | 269 | 0 |
| css/css-nesting | 17 | 13 | 13 | 0 |
| css/css-overflow | 719 | 449 | 449 | 0 |
| css/css-position | 251 | 156 | 161 | 0 |
| css/css-sizing | 568 | 400 | 400 | 0 |
| css/css-text | 1,550 | 1,201 | 1,201 | 0 |
| css/css-text-decor | 339 | 283 | 284 | 0 |
| css/css-transforms | 794 | 748 | 749 | 0 |
| css/css-ui | 962 | 877 | 877 | 0 |
| css/css-values | 192 | 115 | 115 | 0 |
| css/css-writing-modes | 1,162 | 934 | 934 | 0 |
| css/filter-effects | 370 | 281 | 281 | 0 |
| css/selectors | 229 | 203 | 208 | 0 |
| **Total** | **19,627** | **16,398** | **16,427** | **0** |
