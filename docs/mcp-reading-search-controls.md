# Independent reading/search controls

Source-only TEST-ONLY checkpoint from published `fc4c373` (0.1.14). No Rust
build or execution has been performed. This branch changes no implementation,
version, external corpus or historical benchmark data. The tests target the
coordinated RH001v2/RH002 contract; some also preserve behavior already present
in the baseline, including ordinary debug's settling of DOM mutations.

The two local fixtures exercise term-priority window admission with textual
emission order, the 3xwindow excerpt cap, merged overlapping windows, Unicode
lowercase expansion, omission counts and presence after truncation. Link
deduplication retains longer link labels and controls, while role=link keeps
stable refs usable by read and a real fragment navigation. Separate controls
check window/context precedence, numeric open.context rejection before any
browser state change, and exactly one fence around open+find+summary output.

Dynamic reading controls append visible prose outside a UI shell whose hidden
text used to inflate main selection. The public read call must observe fresh
DOM styles/layout after ordinary debug settles; an explicit main ref must keep
its scope. A click-driven
append also checks that article extraction does not remove live form/button
actions from summaries or snapshots. These tests do not require a particular
tag to win selection and do not claim benchmark savings. A final metamorphic
control keeps the volume unchanged after hidden/script source growth, then
adds 300 supplementary-plane Unicode characters to the same selected prose
and checks the volume grows by characters rather than UTF-8 bytes.
