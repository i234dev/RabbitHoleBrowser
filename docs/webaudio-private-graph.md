# Private context graph renderer

This checkpoint is a pure Rust renderer. It opens no device, starts no thread,
changes no existing voice or process mixer, and exposes no JS factory/API.
It depends on the private compressor from `82a92464` and must not be described
as a fix for the reported application until the public integration exists.

The accepted private nodes are buffer source, constant-frequency sine source,
gain, compressor and destination in an acyclic graph. Buffers are already at
the context sample rate; mono is explicitly upmixed to `[s,s]` on ingestion.
Every internal block and destination is canonical stereo. `[s,0]` means a
left-only stereo signal. Other source layouts, resampling/playback/detune/loop
controls, ramps/curves, public channel options and device adapters are not
accepted inputs to this private interface. They have not been implemented or
silently ignored.

## Sample and graph semantics

Each context has a u64 frame cursor. Buffer and sine source start/stop schedules
use exact integer frames, with exclusive stop. A stop before start emits no
samples. Buffer position and oscillator phase depend on that cursor; partial
render calls do not restart either source.

Gain SetValue events are a-rate and apply at their exact frame. Compressor
events contain a full five-parameter snapshot and are k-rate: the renderer
samples them at global frames 0,128,256,... and caches the result throughout
the quantum. A call spanning 127→128 is internally split at128. Stopping a
call at67 cannot move the next k-rate boundary to195. The private timeline
constructor requires ordered, unique event frames; public scheduling and time
conversion are separate future work.

Connections have one valid output/input port where the node kind supplies it.
An identical connection is idempotent. The prepared topological plan validates
all nodes, including disconnected cycles, then executes only the destination's
ancestors. Each node executes once, so fan-out reuses its result. Incoming
blocks are summed before that node's operation, with deterministic connection
order. Compressor delay continues through zero input after a source ends.

Control-side add/remove/connect/disconnect operations are accepted only at
global quantum boundaries. Mid-quantum changes return a typed Boundary error;
they are not deferred or discarded silently. A future control adapter may
prepare/queue changes for a boundary. Failed changes leave edges, execution
plan, current clock and existing DSP state untouched. IDs contain a context
identity and slot generation; foreign/stale IDs cannot address reused nodes.

## Resource and failure boundaries

Limits are 256 arena slots, 2,048 edges, 1,024 preloaded events per timeline
and 64 MiB of accounted node/source/DSP/event payload per context. Structural
plan/edge vectors also have those fixed bounds. This payload budget is not a
claim about total allocator/RSS usage. Shared source buffers are charged
conservatively for every referencing node. Slots with exhausted generations
are retired. There is no unbounded per-frame worklist or allocation in render.

All source PCM and all timeline values, including future events, are checked
before a node is accepted. Float sums/gains use f64 and saturate finite PCM at
`±f32::MAX`; they do not introduce infinity for a valid compressor input.
Render requests contain 1–128 frames and preflight u64 clock overflow.

The requested output is staged and copied to the caller only after all its
segments succeed. An unexpected internal DSP failure faults that context and
publishes no partial new output. It cannot retry partially advanced state,
turn failure into silence, or affect another context. A written corruption
test reaches an invalid second segment deliberately bypassing the validated
control path; ordinary API input cannot install that malformed timeline.

The ten new tests are source-only until root validation: mono/stereo premix,
shared compression versus incorrect per-source compression, stateful fan-out,
gain placement, global/a-rate event boundaries with irregular call sizes,
source schedules/tail, failed mutation atomicity, stale/generation/boundary
guards, finite/budget/clock/context isolation, and defensive late failure.
They do not establish runtime correctness or performance before execution.

## Adapter blocker remains open

There is no realtime or OfflineAudioContext adapter in this checkpoint.
The existing sink consumes device batches while holding its mutex. A universal
two-quantum output queue cannot meet even a 480-device-frame demand when the
context/device rate ratio is high. Before any device/API integration, queue
capacity must follow validated backend demand, rate ratio and resampler margin
under an explicit byte cap, or use a reviewed demand protocol. The context's
suspend/resume clock, queued-data policy, cleanup and JS rooting also require
their own adapter validation. Other media voices must remain independent.
