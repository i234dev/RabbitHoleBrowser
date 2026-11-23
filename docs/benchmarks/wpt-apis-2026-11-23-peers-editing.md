# Web API focused measurements at 10:15 CET on 23 November 2026

The last validated whole-corpus result is 1,338,622/1,773,649 (75.47%) on `d9db0456` across 40,172 browser URLs. The matched main result on `a43c8e0` is 781,820/1,773,653 (44.08%). The independent full run of `367874b4` is in progress. At 10:15 CET its 69 completed native directories covered 23,965 matched URLs. Main passed 342,878/602,405 and the candidate passed 384,348/604,931 on those URLs. The comparison found 15,307 previously passing names absent or failing in that subset. [Previous whole-corpus measurement](wpt-apis-2026-11-23.md)

## Results on identical native URLs

Each row counts named subtests. Gains and losses use the full subtest name and its multiplicity within each URL. A page that reaches its timeout can emit fewer subtests. The reference for every row is `d9db0456`. [Earlier focused results](wpt-apis-2026-11-23-incremental.md)

| Directory or selection | Executable | URLs | Previous passes / named | Candidate passes / named | Gained | Lost |
|---|---|---:|---:|---:|---:|---:|
| webrtc | A | 265 | 26 / 2,339 | 705 / 2,339 | 679 | 0 |
| css/css-color/parsing | A | 46 | 1,240 / 12,188 | 7,685 / 12,188 | 6,445 | 0 |
| editing | A | 727 | 65,957 / 115,080 | 95,076 / 116,143 | 29,342 | 223 |
| css/css-align | B | 242 | 1,695 / 5,435 | 2,635 / 5,435 | 1,010 | 70 |
| WebCryptoAPI/import_export and derive_bits_keys, X25519 selection | B | 8 | 0 / 1,462 | 1,458 / 1,462 | 1,458 | 0 |

Runs used WPT `831baf28d226911e47b2454d69e0c9e7348dd4d1` on TheOtherSide, the Cargo `ci` profile, a loopback-only network namespace, ten workers, CPUs 0–3 and 10–15, and 5 s/15 s timeouts. The frozen executables were built from working trees during development. Their SHA-256 hashes identify the binaries used for the measurements.

| Executable | SHA-256 | Scope |
|---|---|---|
| A | `c02113929a17b9468d6c7a454881418eebeab34419d8bd56fe015bbd9ab5b814` | Browser peer and data channels, layered CSS colors, paragraph and text control editing |
| B | `e163315f63cb2fec313a4c29014894de1cfad81885315712fe244ed58ed14d26` | Includes alignment CSSOM, subgrid feature detection and X25519 |

## Causes and validation scope

- Two local peers exchanged application data through ICE, DTLS and SCTP. The browser integration exercised signaling and a JavaScript data channel round trip. Remote networks, TURN, RTP media and device capture remain unvalidated.
- CSS color parsing recovered every previous named pass in the 46-page comparison. The alignment losses all occur in two pages testing discrete transitions. Separately, some historical interpolation checks had accepted a missing property when the expected and actual elements both returned an empty computed value. The full comparison retains those failures.
- The editing losses include formatting-state queries, selection restoration and differences in the resulting markup. Thousands of additional failures exercise deletion at a paragraph boundary. The engine can delete a neighboring character instead of joining lines with different whitespace styles.
- X25519 passed RFC 7748 key agreement, zero-contribution rejection, non-byte-aligned lengths and import/export access checks. Four WPT setup cases also request an ECDH key that the engine cannot yet import.
- The script library passed 269 unit tests. The grid CSSOM integration group passed nine. Same-origin frame property assignment passed in a group of 22, and the CSS and peer integration group passed 32. The whole-branch CSS pixel comparison, collector stress, test262 and Speedometer controls still refer to the previous validated revision. [Whole-branch control results](wpt-apis-2026-11-23-controls.md)
