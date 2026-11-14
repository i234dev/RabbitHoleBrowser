# RH-AGENT-002: document.write insertion order

The initial TEST-ONLY checkpoint `65bcb10583fede5c586823be56266b4316e922e4`
is based on published 0.1.14
`fc4c3730c64fecdb25acd9c30ffb9c37432a1f74`. Root validated it before
production changes: Script compiled with five failing snapshot controls and
one passing document.open control; HTML compiled with the small-input control
passing and the Unicode compaction control failing. Source hashes were
unchanged before/after each run. Those failures remain preserved separately.

The loading snapshot path currently parses every write as an independent
fragment and recomputes the running script's next sibling each time. That
sibling becomes the first previously written node, reversing subsequent
writes. Recreating the fragment also loses partial-token/open-element state.
Separately, Tokenizer::insert computes its offset before insert_chars compacts
consumed input, so the old offset can point past the surviving stream window.

Six Script controls cover three sequential writes before the original tail,
the same case inside a div, tags/Unicode/entity/CRLF split across calls, inline
nested scripts, separate loading-script positions and the existing
document.open/write/close path. Two HTML controls exercise insertion points,
nested save/restore and subsequent network append, with and without a 70,000
Unicode-character prefix that triggers real input compaction. The unchanged
small-input case is the control for the compaction regression.

The production successor uses a live incremental fragment parser for the
snapshot's writes, anchored once to the running script's parent and original
following sibling. Its tokenizer, open elements and insertion point survive
each write and the outer script's microtasks. Written inline scripts use the
existing nested insertion-point stack. An external blocking script waits
until the outer script/checkpoint finishes, then parsing resumes at its own
point. The pump checks pending scripts before reading the next input slice,
including when the previous slice ended at a blocker.

Only snapshot writers finish automatically at the checkpoint boundary;
document.open-created streams retain their explicit close lifecycle. Closing
a snapshot does not emit readyState/DOMContentLoaded/load again. The normal
fragment streaming API remains separate, including its runScripts policy.
The shared tokenizer now compacts before calculating the insertion offset,
so offsets and saved insertion points use the same character window.

Four additional Script controls cover checkpoint writes/load lifecycle,
pending external scripts, script-created streams across checkpoints and the
existing fragment streaming API. Two Engine controls load actual script
sources through the snapshot loader and check order, parent, Unicode chunks,
nested script execution, microtasks and one load lifecycle. This successor
has not yet been compiled or run; no local Rust execution was used.

This is an adaptation to the existing pre-parsed snapshot loader, not a
replacement with an incremental network document parser. Original page
markup after the script remains the already-built snapshot: an unclosed
written element does not cause that original tail to be reparsed/reparented.
The fix preserves that architecture and does not sort or reorder the
finished DOM to manufacture the requested write order.

Primary contract: [HTML document.write steps](https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#document.write()).
