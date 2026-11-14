# Orthogonal subgrid sizing: staged primitives

This is an isolated successor of `ae5bdeaa1ffd62b8c4a5b0afb292e4120e987aa6`,
**SIN VALIDAR**. It does not fix or activate new orthogonal sizing behavior.
The failed initial 26-case gate and all frozen source snapshots are preserved.

The ordinary track-sizing call now explicitly performs two stages:

1. Initialize tracks and resolve intrinsic bases and growth limits.
2. Maximize, expand flexible tracks and stretch using the actual constraint.

The existing call still runs both stages consecutively with identical inputs
and operations. Two written controls distinguish retained bases from growth
limits and verify later definite-space or max-content completion, including a
spanning flexible track. Neither control has been compiled or run.

This separation is a prerequisite for representing interrupted per-axis sizing.
It does not select the interrupted stage as a final size, replace automatic
sizes with a min-content constraint, or cache partial geometry in LayoutBox.
The outstanding algorithm must define ownership of initial, opposite-axis,
adjusted and final state, then preserve those transitions through wrappers and
scoped inheritance without stale state on remeasure/reparenting. State copies,
restoration and visits remain subject to the existing subgrid work budget.

Source: [Grid2 §12.1](https://drafts.csswg.org/css-grid-2/#algo-overview) requires
subgrid sizing to interlace by axis; orthogonal subgrids invert the usual order.
[Grid2 §12.2](https://drafts.csswg.org/css-grid-2/#algo-terms) keeps definite space,
min-content and max-content constraints distinct. The initial stage is a stage
of an operation, not a new intrinsic constraint.

Release approval still requires the two published critical passes, all eleven
recovered gains, all thirteen retained gains, wider Grid/Flex controls, and
orthogonal/budget regressions. No WPT references or expectations are changed.
