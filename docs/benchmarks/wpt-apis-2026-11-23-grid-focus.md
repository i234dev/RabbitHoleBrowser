# Web API validation at 11:10 CET on 23 November 2026

The last complete comparison on the 40,172 fixed URLs passed 1,338,622/1,773,649 subtests (75.47%) on `d9db0456`. Main at `a43c8e0` passed 781,820/1,773,653 (44.08%). A subsequent full run at `367874b4` had completed 169 directories and 29,352 matched URLs by 11:09. Main passed 591,578/1,428,801 and that candidate passed 1,190,082/1,431,359 on those URLs. The comparison found 15,349 previous named passes missing or failing in the partial run. [Whole-corpus results by directory](wpt-apis-2026-11-23-directories.md)

## Focused comparisons

The reference for this table is `d9db0456`. Each row compares all named subtests recorded on the same URLs, preserving duplicate names. The executed count can change when a setup succeeds or a timeout occurs. TheOtherSide ran the local WPT checkout at `831baf28d226911e47b2454d69e0c9e7348dd4d1` with ten workers bound to CPUs 0–3 and 10–15. Each runner used Cargo’s ci profile and a loopback-only namespace, with 5-second timeouts and 15 seconds for long tests.

| Selection | URLs | Earlier passes / subtests | Later passes / subtests | Gained | Lost |
|---|---:|---:|---:|---:|---:|
| css/cssom | 272 | 2,812 / 4,458 | 3,135 / 4,458 | 323 | 0 |
| css/css-grid/animation and css/css-grid/grid-lanes/animation | 16 | 1,692 / 3,784 | 3,626 / 3,784 | 1,934 | 0 |
| css/css-align | 242 | 1,695 / 5,435 | 2,705 / 5,435 | 1,010 | 0 |
| editing | 727 | 65,957 / 115,080 | 101,749 / 116,144 | 36,009 | 217 |
| webrtc | 265 | 26 / 2,339 | 720 / 2,339 | 694 | 0 |
| WebCryptoAPI, NIST ECDH import/export and derivation | 12 | 0 / 2,378 | 2,128 / 2,420 | 2,128 | 0 |
| html/dom, three ARIA reflection pages | 3 | 445 / 1,805 | 645 / 1,805 | 209 | 9 |

The CSSOM runner included `91163fe4`, which fixed nullable priority conversion. The grid and ARIA executable included `72df8e5b` and `ab4be386`, with SHA-256 `8aa62f216ef105b5fc723258bc28dfd4ee7c201c394b704b093eacef10223304`. The ECDH executable included `138d3863`, with SHA-256 `f6a8546732bd091b9a9737e5d25c46aa3b2616f514db718d6812b7f4abdcefc7`.

## Remaining causes and risks

- Editing failures include selection restoration, formatting-state queries and DOM serialization after a paragraph join. Preserved text line breaks now delimit joins. Caret placement after a join still needs coverage across inline styles and bidirectional selections.
- Grid interpolation uses the repeat count and track structure. Interpolating an automatic repeat as if it had a fixed count had lost 80 earlier passes. Switching these tracks discretely recovered all 80. CSS keyframe composition accounts for additional failures.
- WebRTC statistics count actual wire activity and data-channel opens and closes. The integration test exchanged a five-byte message and a six-byte reply over ICE, DTLS and SCTP. This measured data channels on the local host. Media, devices and relay operation require their own validation.
- Four ECDH setup cases request ECDSA keys in addition to the implemented agreement keys. The backend validates curve compatibility and the WebCrypto binding checks key usage and export permissions.
- Stable ARIA reflection passed all 41 cases. The nine losses on the tentative enumerated page expect a string for an absent Busy, Current, Disabled, Hidden, Invalid, Live, Modal, Required or Sort property. The stable nullable reflection returns null, whose JavaScript type is object.

## Independent controls

Both test262 modes at `452f5495` passed 46,017 of 46,064 executed cases, failed 47 and skipped 7,614. Both reported zero regressions against the committed baseline. The test262 revision was `2e58f87d8350ad51224617a041c6e041d52f8444` with six workers per mode, and the forced mode enabled the JIT with threshold zero. At `ab4be386` all 41 engine regression tests and all 11 web animation integration tests passed. An aligned Speedometer A/B and a full unit and collector-stress run covering the latest changes are pending. [Earlier control results](wpt-apis-2026-11-23-controls.md)

The complete CSS pixel comparison on `24cb56cd` passed 16,419/19,627 against main’s 16,398/19,627. Exactly 21 pages became passes and every previous pass was preserved. The candidate recorded 3,177 failures and 31 errors. The renderer SHA-256 was `90df3427a2902e48f24c81d7369332a62a86ff49b8d019f897e9b34c0293078d`. The runner used four workers, a 20-second timeout and mouse actions disabled. The table groups the identical paths in the two raw result files by CSS directory.

| CSS directory | Pages | Main passes | Candidate passes | Gained | Lost |
|---|---:|---:|---:|---:|---:|
| css/CSS2 | 6,232 | 5,782 | 5,783 | 1 | 0 |
| css/compositing | 60 | 36 | 36 | 0 | 0 |
| css/css-align | 64 | 37 | 37 | 0 | 0 |
| css/css-backgrounds | 718 | 607 | 607 | 0 | 0 |
| css/css-borders | 123 | 42 | 42 | 0 | 0 |
| css/css-box | 42 | 38 | 38 | 0 | 0 |
| css/css-cascade | 51 | 41 | 41 | 0 | 0 |
| css/css-color | 311 | 291 | 295 | 4 | 0 |
| css/css-conditional | 172 | 149 | 150 | 1 | 0 |
| css/css-contain | 435 | 307 | 307 | 0 | 0 |
| css/css-display | 223 | 179 | 179 | 0 | 0 |
| css/css-flexbox | 1,027 | 875 | 879 | 4 | 0 |
| css/css-fonts | 387 | 262 | 262 | 0 | 0 |
| css/css-grid | 1,763 | 1,390 | 1,396 | 6 | 0 |
| css/css-images | 482 | 434 | 434 | 0 | 0 |
| css/css-masking | 384 | 268 | 269 | 1 | 0 |
| css/css-nesting | 17 | 13 | 13 | 0 | 0 |
| css/css-overflow | 719 | 449 | 449 | 0 | 0 |
| css/css-position | 251 | 156 | 156 | 0 | 0 |
| css/css-sizing | 568 | 400 | 400 | 0 | 0 |
| css/css-text | 1,550 | 1,201 | 1,201 | 0 | 0 |
| css/css-text-decor | 339 | 283 | 284 | 1 | 0 |
| css/css-transforms | 794 | 748 | 748 | 0 | 0 |
| css/css-ui | 962 | 877 | 877 | 0 | 0 |
| css/css-values | 192 | 115 | 115 | 0 | 0 |
| css/css-writing-modes | 1,162 | 934 | 934 | 0 | 0 |
| css/filter-effects | 370 | 281 | 281 | 0 | 0 |
| css/selectors | 229 | 203 | 206 | 3 | 0 |
