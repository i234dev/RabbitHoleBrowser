# Named-line occurrences in the four auto-fit gates

This candidate starts from published 0.1.16, revision
`90e464f3c603617b718b9bd41568e48d79ddccd5`. It has not been compiled or
executed. It changes placement; it does not claim that the four WPT gates pass.

The fixed corpus is WPT `831baf28d226911e47b2454d69e0c9e7348dd4d1`:

- `css/css-grid/grid-lanes/subgrid/grid-subgridded-to-grid-lanes/track-sizing/column-subgrid-auto-fit-001.html`
- `css/css-grid/grid-lanes/subgrid/grid-subgridded-to-grid-lanes/track-sizing/row-subgrid-auto-fit-001.html`
- `css/css-grid/subgrid/parent-repeat-auto-fit-001.html`
- `css/css-grid/subgrid/parent-repeat-auto-fit-002.html`

Root's recorded comparison of published 0.1.15 (`2eb24ff`) with the CSS source
delivered in 0.1.16 (`f9a1f7e`) has four FAIL results on both sides. The candidate
pixel differences were respectively 69944, 90730, 67973 and 88573 pixels.
These remain historical failures; their tests, references and expectations are
unchanged. The receipt is `autoFit4-f9a-ci24-attempt1/receipt.json` under the
preserved `grid-b-cc7-composition-20261114/final15-baseline-readiness/runtime`
evidence directory.

## Cause and change

All four pages use placements such as `y 5` and `y -1`. The parser discarded
`y` whenever an integer was present, so layout counted every line rather than
the lines named `y`. That changes occupied tracks and therefore which auto-fit
tracks collapse. Bare named spans also previously became a numeric span of one
even when an opposite line was available.

The computed representation now retains the name, ordinal and span flag.
Placement counts each matching line once, searches backwards for negative
ordinals and continues onto implicit lines on the appropriate side. Named spans
count from their opposite edge. Explicit `y 1` stays distinct from bare `y`,
which first checks the implicit area edge name. The same resolver is used by
ordinary Grid, grid lanes, subgrids and the existing absolute placement path.

The parser rejects zero/fractional line numbers, negative spans, duplicate
components and mixed `auto`, preserving the previous declaration. Shorthand
defaults keep an explicit ordinal separate from a bare area name.

The contract follows [Grid2 §8.3](https://drafts.csswg.org/css-grid-2/#grid-placement-properties)
and [§8.3.1](https://drafts.csswg.org/css-grid-2/#grid-placement-errors).
Auto-fit occupancy remains the existing algorithm described in
[Grid2 §7.2.3.2](https://drafts.csswg.org/css-grid-2/#auto-repeat) and the
[Grid3 lanes heuristic](https://drafts.csswg.org/css-grid-3/#repeat-auto-fit).
No track solver, repetition count or collapse rule is changed in this patch.

## Controls pending execution

There are ten new Rust tests: three style tests, three resolver unit tests,
three layout tests and one Engine reftest. The layout tests exercise Grid and
lanes, rows and columns, inherited local names, collapsed parent tracks and
named spans across unequal physical tracks. The Engine fixture contains eight
panels; its reference uses positioned Block boxes with independently specified
geometry and no Grid/Flex. The fixtures do not call `getComputedStyle()`.

Required validation is the new tests and reftest, existing Grid/layout parity,
the unchanged historical 26 cases, mix2 and all four auto-fit gates. No results
are inferred from static inspection.

## Remaining blocker in the original four pages

Each WPT page appends a visible error report when the resolved
`grid-template-columns` or `grid-template-rows` differs from its expected used
track list. The shipped CSSOM implementation returns an empty string for these
properties, visible in the preserved PNGs. Correct placement alone therefore
cannot make those pages pass. A later, separate bridge must expose the actual
used tracks and line names from the completed layout to CSSOM; reconstructing
them from authored CSS or suppressing the report would not meet that contract.

That bridge is a proposal, not part of this candidate. The four original pages
remain validation inputs with their reports and references intact.
