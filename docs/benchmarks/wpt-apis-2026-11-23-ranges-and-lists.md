# Web API and CSS checks at 13:00 CET on 23 November 2026

The latest complete result on the fixed 40,172 URLs is 1,338,622/1,773,649 (75.47%) at `d9db0456`. Main at `a43c8e0` passed 781,820/1,773,653. On the 34,789 matched URLs available across 220 directories at `367874b4`, main passed 761,922/1,719,525 and the candidate passed 1,447,319/1,721,946. That partial comparison has 15,647 previously passing named subtests absent or failing. Separate runs at different revisions must be compared on their own URL sets. [Last complete comparison by directory](wpt-apis-2026-11-23-directories.md)

## Browser API checks

TheOtherSide used WPT `831baf28d226911e47b2454d69e0c9e7348dd4d1` in an isolated loopback network namespace, Cargo's ci profile, 10 workers and a 5,000 ms page timeout. Counts below match named subtests by URL and name, preserving duplicate names. The earlier focus executable had SHA-256 `d8468677873c0f7357162e93fb10b2edd5506cc4dca99f4c4917dc90626540d0`. The later one had SHA-256 `8440f9bad76ed9dd296398b8d33056a41a5f992dd6320b993dcd5ea60a6fdafc` and included the changes recorded through `e30b4450`. Different rows cover different executables.

| Directory and selection | Pages | Reference | Candidate | Gained | Lost |
|---|---:|---:|---:|---:|---:|
| css/cssom, all, versus d9db0456 | 272 | 2,812/4,458 | 3,586/4,458 | 774 | 0 |
| css/css-color/parsing and css/css-images/parsing, versus 32d18bea | 71 | 8,330/15,024 | 10,337/15,024 | 2,007 | 0 |
| html/semantics/popovers, versus d9db0456 | 94 | 1,683/3,920 | 3,535/3,921 | 1,852 | 0 |
| html/rendering, list defaults, headings and form resets, versus d9db0456 | 6 | 5,112/5,116 | 5,116/5,116 | 4 | 0 |
| dom/ranges, insertNode and surroundContents, before and after detached document sharing | 2 | 1,317/3,680 | 3,022/3,680 | 1,709 | 4 |
| editing, all, versus d9db0456 | 727 | 65,957/115,080 | 106,286/116,294 | 40,560 | 231 |
| editing/run, indent and outdent after the caret-boundary fix | 4 | 3,703/3,879 | 3,704/3,879 | 1 | 0 |
| webrtc, all, versus d9db0456 | 265 | 26/2,339 | 836/2,340 | 810 | 0 |

The four range losses affect the endpoint after trying to insert a document or doctype at a text boundary in another document. The algorithm split the text before validating the node. Shared detached documents preserve wrapper identity within each receiving realm, survive removal of the originating iframe, and keep range endpoints in the original document arena. The frame suite passed 24/24 and engine regressions passed 53/53. The WebRTC receiver test checks delivery of a Blob followed by text, including when the sender immediately closes the channel.

Rich editing still has failures in whitespace joins, deletion across paragraphs and list-state queries with nested lists. In the latest full editing replay, list and indentation changes gained 637 and lost nine named subtests against the preceding editing run. The caret-boundary fix subsequently recovered one of those nine. WebRTC certificate generation and the SCTP data path were exercised on Linux. The Windows certificate backend, RTP media capture and complete ICE-TCP negotiation have separate coverage requirements. The performance and test262 figures in the earlier report predate these changes. [Earlier controls](wpt-apis-2026-11-23-physical-geometry.md)

## CSS reftests by directory

The 12:44 CET reftest executable included the HTML list, heading and input defaults before the final `type=none` adjustment. Its SHA-256 was `e28510ea0c977428718d50a169dfce2d263fd1f24efacb2070080cdf070e4998`. TheOtherSide ran 10 workers with a 20-second timeout and mouse actions disabled. Every route occurred exactly once in both result files. Main passed 16,398/19,627, and this build passed 16,435/19,627. All 37 changes were gains.

| Directory | Routes | Main passed | Candidate passed | Gained | Lost |
|---|---:|---:|---:|---:|---:|
| css/CSS2 | 6,232 | 5,782 | 5,783 | 1 | 0 |
| css/compositing | 60 | 36 | 36 | 0 | 0 |
| css/css-align | 64 | 37 | 37 | 0 | 0 |
| css/css-backgrounds | 718 | 607 | 607 | 0 | 0 |
| css/css-borders | 123 | 42 | 42 | 0 | 0 |
| css/css-box | 42 | 38 | 38 | 0 | 0 |
| css/css-cascade | 51 | 41 | 41 | 0 | 0 |
| css/css-color | 311 | 291 | 300 | 9 | 0 |
| css/css-conditional | 172 | 149 | 150 | 1 | 0 |
| css/css-contain | 435 | 307 | 307 | 0 | 0 |
| css/css-display | 223 | 179 | 179 | 0 | 0 |
| css/css-flexbox | 1,027 | 875 | 879 | 4 | 0 |
| css/css-fonts | 387 | 262 | 262 | 0 | 0 |
| css/css-grid | 1,763 | 1,390 | 1,397 | 7 | 0 |
| css/css-images | 482 | 434 | 436 | 2 | 0 |
| css/css-masking | 384 | 268 | 269 | 1 | 0 |
| css/css-nesting | 17 | 13 | 13 | 0 | 0 |
| css/css-overflow | 719 | 449 | 449 | 0 | 0 |
| css/css-position | 251 | 156 | 161 | 5 | 0 |
| css/css-sizing | 568 | 400 | 400 | 0 | 0 |
| css/css-text | 1,550 | 1,201 | 1,201 | 0 | 0 |
| css/css-text-decor | 339 | 283 | 284 | 1 | 0 |
| css/css-transforms | 794 | 748 | 749 | 1 | 0 |
| css/css-ui | 962 | 877 | 877 | 0 | 0 |
| css/css-values | 192 | 115 | 115 | 0 | 0 |
| css/css-writing-modes | 1,162 | 934 | 934 | 0 | 0 |
| css/filter-effects | 370 | 281 | 281 | 0 | 0 |
| css/selectors | 229 | 203 | 208 | 5 | 0 |
| **Total** | 19,627 | 16,398 | 16,435 | 37 | 0 |
