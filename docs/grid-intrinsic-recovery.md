# Grid intrinsic recovery: diagnostic checkpoint

This branch is **SIN VALIDAR** and is not a release candidate. It depends on the
subgrid metadata, explicit GridSpace constraints and normal-gap provenance
commits. No Rust build, runtime test or remote synchronization was performed by
this agent.

The first inheritance checkpoint transports immutable parent track sizes,
positions, collapsed state, line names and gaps to a requested subgrid axis.
Writing-mode wrappers map and reverse those axes. Scoped installation restores
computed styles and invalidates caches. The child cannot size inherited tracks
or append implicit tracks, and its own edge layer is applied separately from the
parent track geometry. Six pure unit tests describe this geometry and restoration.

This is an interface checkpoint, **not complete subgrid support**. In particular,
projecting descendant contributions into parent tracks, phase-aware orthogonal
measurement, intrinsic row-lane recursion, inherited state during stacking-axis
stretch relayout, and exhaustive nested edge/placement behavior remain pending.
Those paths must be completed in a subsequent commit before integration. The
current partial wiring must not be cherry-picked into a published version alone.

Required gates remain unchanged: restore the eleven deferred intrinsic gains,
retain the thirteen improvements shipped in 0.1.9 and preserve both previously
passing subgrid controls (row grid-lanes-subgrid-005a and
column-subgrid-orthogonal-writing-mode-002). Four other auto-fit failures already
present in the stable control are not to be hidden or assigned new expectations.
No baseline or WPT corpus file is changed.

Normative algorithm references (drafts; no complete conformance claim):

- <https://drafts.csswg.org/css-grid-2/#algo-overview>
- <https://drafts.csswg.org/css-grid-2/#subgrids>
- <https://drafts.csswg.org/css-grid-3/#subgrids>
- <https://drafts.csswg.org/css-grid-3/#intrinsic-sizes>
- <https://drafts.csswg.org/css-writing-modes-3/#orthogonal-flows>
