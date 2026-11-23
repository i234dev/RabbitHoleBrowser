# Web API checks on 23 November 2026 at 14:55 CET

The JavaScript and JIT sources in this run matched revision `eed3dd5a`. Both modes passed 46,017 of the 46,064 executed test262 files. The file-by-file comparison recorded zero regressions and zero newly passing files against the tracked baseline. The full WPT result available on this date was 82.63% at `367874b4` across 40,172 URLs. [Complete API results by directory](wpt-apis-2026-11-23-367874b4.md)

## JavaScript conformance

| Revision | Corpus revision | Mode | Passed | Failed | Skipped | Runtime |
|---|---|---|---:|---:|---:|---:|
| eed3dd5a | 2e58f87d8350ad51224617a041c6e041d52f8444 | Normal | 46,017 | 47 | 7,614 | 50 s |
| eed3dd5a | 2e58f87d8350ad51224617a041c6e041d52f8444 | JIT forced, threshold 0 | 46,017 | 47 | 7,614 | 61 s |

TheOtherSide ran the same executable in both modes with ten workers on CPUs 0–3 and 10–15 inside an isolated network namespace. Cargo used profile `ci`, the locked dependency graph and the `rabbithole-js/jit` feature. The build guard and Cargo both exited with status 0. The executable had SHA-256 `be0081af919d7bdc73352a760dd28fd9838b7d8c3d8fa74fb48132b2a53500a9`.

## Focused native WPT comparisons

Both sides of each row used the same URLs in WPT `831baf28d226911e47b2454d69e0c9e7348dd4d1`. The native testharness runner used the ci profile, ten workers, isolated loopback and 5 s ordinary / 15 s long timeouts. The rows count named subtests, matching duplicate names by multiplicity. The ChaCha20-Poly1305 row is part of the WebCrypto row.

| Directory and reference | URLs | Earlier pass / total | eed3dd5a pass / total | Gained | Lost |
|---|---:|---:|---:|---:|---:|
| css/css-anchor-position, scoped names before anchor lengths | 268 | 8,020 / 13,662 | 11,910 / 13,662 | 3,902 | 12 |
| editing, hyperlink commands before the subscript and superscript changes | 727 | 107,734 / 116,354 | 107,898 / 116,349 | 164 | 0 |
| WebCryptoAPI, 367874b4 | 333 | 31,128 / 69,077 | 46,396 / 69,735 | 15,268 | 0 |
| WebCryptoAPI, ChaCha20-Poly1305 pages at 367874b4 | 10 | 0 / 1,044 | 1,040 / 1,044 | 1,040 | 0 |

The twelve anchor losses comprise four nested calculation serializations, three inline geometry cases, one multicolumn fragment, two shadow tree scope cases, one combined fallback tactic and one observer notification. The editing delta covers selection boundaries and mutually exclusive script formatting. The WebCrypto delta includes multiple key algorithms landed since the complete run. The ChaCha20-Poly1305 control exercises authenticated encryption, rejection of an altered tag, JWK import and export, key wrapping, and key derivation.

At this checkpoint all 64 engine regression tests and the dedicated ChaCha20-Poly1305 test passed. The full CSS reftest comparison on the earlier anchor-length implementation recorded 16,435 / 19,627, with 37 gains and zero losses against main. The full collector-stress and Speedometer results for the current branch remain open. The disk guard terminated the subsequent position-try build when free space crossed the 15 GiB floor. Its output was excluded from the comparisons.
