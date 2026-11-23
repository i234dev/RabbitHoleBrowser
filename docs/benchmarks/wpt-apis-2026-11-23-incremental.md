# Web API implementation checks, 23 November 2026

At 09:22 CET the last completed whole-corpus audit was **75.47%** on `d9db0456`, with 1,338,622 passes out of 1,773,649 reported results on 40,172 browser URLs. The matching published-main run at `a43c8e0` passed 781,820/1,773,653 (44.08%). The next whole-corpus run on `367874b4` started at 08:02:51 UTC. [Previous whole-corpus measurement and validation](wpt-apis-2026-11-23.md)

## Paired implementation checks

Each row compares the same native URLs with the saved `d9db0456` results. Entries count named subtests actually reported. A timeout can change the denominator. Gains and losses compare the multiset of full names within each browser URL. Subsets overlap. The canonical corpus audit separately accounts for pages emitting no subtests, assigned server ports and the upstream-server URLs.

| Area or selection | Candidate | URLs | Previous passes / named | Candidate passes / named | Gains | Losses |
|---|---|---:|---:|---:|---:|---:|
| wasm | 81c87357 | 509 | 107,508 / 191,676 | 191,012 / 191,676 | 83,505 | 1 |
| WebCryptoAPI | e33ec51e | 333 | 15,246 / 67,300 | 31,086 / 69,073 | 15,979 | 139 |
| WebCryptoAPI key derivation selection | 2becfc01 | 38 | 6,085 / 23,007 | 19,336 / 24,734 | 13,251 | 0 |
| WebCryptoAPI/encrypt_decrypt | 6c952cd8 | 12 | 831 / 2,811 | 1,834 / 2,938 | 1,003 | 0 |
| editing | 367874b4 | 727 | 65,957 / 115,080 | 89,040 / 115,998 | 23,286 | 203 |
| input-events | 5f2e9975 | 34 | 174 / 3,252 | 1,260 / 3,107 | 1,103 | 17 |
| html/dom and focus selection | 4a014b09 | 260 | 57,182 / 72,434 | 65,850 / 72,434 | 8,670 | 2 |
| shadow-dom | f2f5c3a0 | 315 | 5,521 / 9,936 | 9,052 / 9,936 | 3,531 | 0 |
| custom-elements | f2f5c3a0 | 200 | 2,921 / 4,205 | 2,933 / 4,205 | 12 | 0 |
| CSS positioning selection | 4f73a8e9 | 7 | 1,092 / 1,836 | 1,348 / 1,836 | 256 | 0 |
| CSS feature-query selection | e33ec51e | 12 | 1,445 / 1,643 | 1,609 / 1,643 | 174 | 10 |
| css/css-color/parsing | a993c15d | 46 | 1,240 / 12,188 | 5,939 / 12,188 | 4,699 | 0 |
| css/css-color/parsing | 6c952cd8 | 46 | 1,240 / 12,188 | 7,463 / 12,188 | 6,410 | 187 |
| WebRTC ICE candidate constructor | 4f73a8e9 | 1 | 0 / 65 | 65 / 65 | 65 | 0 |
| WebRTC ICE events and errors | f2f5c3a0 | 4 | 0 / 30 | 30 / 30 | 30 | 0 |

The runs used WPT `831baf28d226911e47b2454d69e0c9e7348dd4d1` on TheOtherSide with a loopback-only network namespace. Each native run used the Cargo `ci` executable, ten workers, CPUs 0–3 and 10–15, 5 s or 15 s timeouts and a 2,500,000 KiB per-process virtual memory limit. The matching JSONL, logs, build-budget reports and the full-run binary checksum are retained in the measurement directory `wpt-api-90`.

## Failure analysis and validation scope

* WebCrypto's earlier lost passes cluster in AES-GCM pages timing out before finishing their assertions. Replacing interpreted field multiplication and AES with native primitives recovered the previous passing names in the encryption directory. The NIST example covers a variable-length IV and truncated tag. Authentication is checked before returning plaintext.
* In CSS, 160 previous passes were lost on the specified syntax of layered colors and 27 on alpha replacement. CSS feature-query losses concern five details-content selector cases, four picker selector cases and one nested at-rule. Computed style inheritance, currentColor, shorthand reset and alpha serialization have dedicated tests.
* Editing still loses cases involving selection restoration, mixed inline formatting, command enablement in text controls and persistent editing preferences. Format changes can split text nodes and their inline ancestors. Integration tests cover backward selections, typing at the caret, existing siblings and cancellation of keyboard edits.
* Of the two HTML losses in the focused run, one checks the required argument count of ShadowRoot.setHTMLUnsafe. The other checks first-paint timing against render-blocking loads. Shadow-root metadata and HTML reflection have separate tests.
* ICE events and signaling objects passed the cases shown above. The table contains no peer transport score.

The most recent completed CSS reftest comparison, two test262 modes, unit and collector-stress runs and paired Speedometer run apply to `d9db0456`. Those controls must run again on the delivery revision. [Previous control results](wpt-apis-2026-11-23-controls.md)
