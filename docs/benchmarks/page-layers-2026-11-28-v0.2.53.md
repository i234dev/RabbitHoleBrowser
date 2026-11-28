# Fixed and sticky page layers: 0.2.53 (28 November 2026)

## Change

0.2.52 kept a viewport-fixed suffix of the display list in a separate layer
while the interface scrolls ahead of the page's repaint, but fell back to a
single canvas whenever an element had focus. On a real news site the
consent dialog focused a button and removed it; the content thread kept
the removed node as focused, so the header moved with every scroll for the
rest of the visit, as it did after any click on a link.

0.2.53 splits the display list into the page canvas followed by up to 16
layers in paint order (`DisplayList::layer_plan`): `position: fixed` groups
(composited in viewport coordinates), page-scrolled `position: sticky` boxes
(placed for the presented scroll position from their sticky constraints,
including the end of their container and scrolling back up) and the
scrolling content painted between or above them. The focus ring, caret and
find highlights are a layer of their own instead of disabling the others,
and a removed element no longer counts as focused. Effects that read the
pixels below them after the first layer (`backdrop-filter`,
`mix-blend-mode`), fixed or sticky boxes nested in a clip, transform or
opacity group, sticky boxes in inner scroll containers and plans with more
than 16 layers keep the single-canvas behaviour.

## Measurements

On the real page (28 November 2026, TheOtherSide, WSL2 Linux x86-64,
content thread probe at scale 1.25, 20 scroll steps, with the removed
focused element and with a focused link): 0 differing pixels in the 47 px
header band between frames presented ahead of the repaint and repainted
frames.

App 158, layout 368 and engine 346 tests pass (917 in the release run);
CSS reftests 16,578 / 19,627 with no differences.

A page whose content is mostly painted above a fixed header (header first
in the document, content in a relatively positioned wrapper) repaints in
about 16 ms instead of 7 ms at 1280×800 and scale 1.25, because that content
is rastered as a layer; pages without content above their fixed boxes are
unaffected.

## Speedometer 3.1

Paired runs on 28 November 2026 (Europe/Madrid) on TheOtherSide, CPU 8,
8 repetitions, same method as the
[0.2.48 record](speedometer-2026-11-26-v0.2.48.md): A = 0.2.52 integration
`9e63f140` (sha256 `5124d0db…`), B = 0.2.53 integration `a2c9c2fa`
(sha256 `bc82b0a7…`).

| Rep | A geomean (ms) | B geomean (ms) | A/B |
|---|---:|---:|---:|
| 1 | 191.67 | 191.46 | 1.001 |
| 2 | 190.42 | 190.79 | 0.998 |
| 3 | 191.76 | 187.69 | 1.022 |
| 4 | 190.41 | 190.20 | 1.001 |
| 5 | 192.40 | 190.92 | 1.008 |
| 6 | 190.99 | 188.94 | 1.011 |
| 7 | 190.03 | 190.41 | 0.998 |
| 8 | 190.41 | 189.43 | 1.005 |

Median **1.003×** (6 of 8 pairs): no measurable change.
