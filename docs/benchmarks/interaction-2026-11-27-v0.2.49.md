# DOM interaction on large pages: 0.2.49 against 0.2.48 (27 November 2026)

Cost of one small DOM change followed by a layout read, on real pages, measured
on 27 November 2026 on TheOtherSide (Intel i7-12700H, WSL2 Linux x86-64), one
core (CPU 8), with the alignment-normalized release builds of the `shot` example
used for the Speedometer comparisons: A = 0.2.48 (`777c1b7e`, sha256
`f5e559aa…`), B = 0.2.49 integration (sha256 `30024d3a…`). Each page was loaded
live, then a script ran each operation 30 times (10 for the body class) and
reported the mean time per operation; the table gives the median of three runs.

Operations, on the middle `p`/`li`/`div` element of the page unless noted:
`class` toggles a class no rule uses, `text` appends text, `append` appends a
list item to a list inserted at the top of the body, `style` changes
`style.color`, `body class` toggles a class on `body`. Every iteration reads
`getBoundingClientRect()` and `offsetHeight` of the body.

| Page (elements) | Operation | 0.2.48 (ms) | 0.2.49 (ms) |
|---|---|---:|---:|
| CSS reference page, developer documentation (8,502) | class | 30.75 | 0.08 |
| | text | 1.54 | 0.83 |
| | append | 31.02 | 0.61 |
| | style | 1.68 | 0.93 |
| | body class | 32.02 | 0.07 |
| Repository page, code hosting site (2,468) | class | 0.84 | 0.37 |
| | text | 31.15 | 2.47 |
| | append | 30.88 | 4.30 |
| | style | 1.70 | 3.54 |
| | body class | 16.75 | 0.29 |
| Long encyclopedia article (21,945) | class | 11.57 | 0.31 |
| | text | 10.33 | 6.69 |
| | append | 5.17 | 2.74 |
| | style | 10.12 | 8.03 |
| | body class | 3.23 | 0.29 |
| News aggregator front page (804) | class | 0.36 | 0.02 |
| | text | 0.34 | 0.36 |
| | append | 0.08 | 0.05 |
| | style | 0.35 | 0.36 |
| | body class | 0.08 | 0.02 |

The `style` change on the repository page is slower (1.70 → 3.54 ms) and is
under investigation. Live pages change between loads; the numbers are
indicative, not a controlled benchmark.

## What changed

- `:has()` in a compound that styles other elements than its subject made any
  mutation that could reach its argument restyle and relayout the whole
  document. The subjects' `:has()` results are cached and re-evaluated after a
  mutation; only subjects whose result flipped restyle the related elements.
- Attribute selectors restyle by the selectors whose match differs between the
  old and new value, instead of by every selector on that attribute name.
- Attribute changes that change no style no longer damage layout.
- The `:visited` link set is rescanned only when links change; list item
  numbers are computed once per list.
- Query containers: an inline-size container's block size is not recorded, and
  a container change restyles only its subtree.

## Speedometer 3.1

Paired runs on 27 November 2026, same method as the
[0.2.48 record](speedometer-2026-11-26-v0.2.48.md), A = 0.2.48, B = 0.2.49
integration: median **1.0006×** (4 of 8 pairs; range 0.949–1.011). An earlier
run of the integration before the last invalidation fixes measured 1.0034×
(5 of 8). Speedometer's workloads rebuild their DOM between steps; the changes
above target interaction on large existing pages.
