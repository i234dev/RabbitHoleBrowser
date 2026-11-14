# RH-AGENT-002: document.write insertion order

This TEST-ONLY checkpoint is based on published 0.1.14
`fc4c3730c64fecdb25acd9c30ffb9c37432a1f74`. It has not been compiled or run.
Production parser, tokenizer, snapshot loading and streaming code is unchanged.

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

The production successor must preserve an incremental insertion point and
parser state across writes; it must not repair order by sorting/reordering
the finished DOM. Snapshot-created writing state must remain scoped to the
document/script and must not duplicate page load events or replace the
existing script-created/streaming parser behavior. Root will preserve the
before failure and run the same controls after the fix.

Primary contract: [HTML document.write steps](https://html.spec.whatwg.org/multipage/dynamic-markup-insertion.html#document.write()).
