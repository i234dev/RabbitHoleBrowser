# Private compressor DSP core

This module is not a public Web Audio implementation. It adds no factory,
constructor, audio device or change to the process mixer. The context graph
and JS integration must be delivered separately before exposing a compressor.

`audio::compressor::Compressor` owns one node's persistent state. Input is a
block of 1–128 stereo frames containing the **already mixed** node input.
Mono may use `[sample, 0]`. Channel conversion is the graph renderer's job.
One instance must never be shared between nodes or contexts. There is no
allocation in `process`; construction allocates at most 4,610 stereo frames
(36,880 sample bytes) at the upper supported rate of 768 kHz. Supported rates
are 3–768 kHz, matching the existing AudioBuffer bounds.

## Contract and implementation choices

The [W3C Recommendation, §1.19](https://www.w3.org/TR/2021/REC-webaudio-20210617/#DynamicsCompressorNode)
specifies five k-rate parameters, persistent detector/gain state, look-ahead,
makeup and metering. Their defaults/ranges are:

| Parameter | Default | Inclusive range |
| --- | ---: | --- |
| threshold, dB | -24 | -100 to 0 |
| knee, dB | 30 | 0 to 40 |
| ratio | 12 | 1 to 20 |
| attack, seconds | 0.003 | 0 to 1 |
| release, seconds | 0.25 | 0 to 1 |

The [processing contract](https://www.w3.org/TR/2021/REC-webaudio-20210617/#dynamicscompressornode-processing)
permits choices of smooth curve shapes. This implementation uses:

* A stereo-linked peak detector and continuous identity detector-rate curve.
* A quadratic knee in dB from threshold to threshold+knee, with unit slope
  below and slope 1/ratio above, joined continuously. Knee zero is hard knee.
* Adaptive log-domain envelopes. A reference 20 dB gain step changes 10 dB
  in attack/release seconds. Zero time makes that stage immediate.
* A fractional, linearly interpolated 6 ms delay, retaining source-end tail.
* Makeup `(1 / curve(1))^0.6` and block-end metering including that gain.
  A fully silent, drained block reports zero; low-level active signals can
  have positive metering gain because makeup is included.

These equations are implementation choices, not another browser's algorithm
or a claim of sample-for-sample compatibility. In particular, times are
calibrated on a stated reference transition rather than a constant dB slew.

For input level `x`, threshold `T`, knee width `K`, ratio `R`, write `u=x-T`.
The output level in dB is:

```
x                                      u <= 0
x + (1/R - 1) * u*u / (2*K)            0 < u < K
T + K*(1 + 1/R)/2 + (u-K)/R            u >= K
```

Ratio one is explicitly the identity, including floating-point evaluation.
For each sample, absolute input below 0.0001 selects attenuation one; other
inputs use `curve(peak)/peak`. The detector average starts at zero and updates
by `(attenuation-average)*attenuation`. Compressor gain starts at one.

Given `N = seconds * sampleRate`, define `b = 1 - 2^(-1/N)` (one when time is
zero). For `q = compressorGain/detectorAverage`, the attack interpolation rate
is `(q - q^(1-b))/(q-1)`, with its continuous limit `b` at `q=1`.
It is increasing in `q` and lies in `[0,1]`. The release multiplier is
`q^(-b)`, decreasing in `q` and greater than one for `0<q<1`.
If the smoothed detector has not caught up to the current gain during a
release transition, release waits rather than decreasing the gain. The meter
is updated only after a successful complete block.

## Defensive boundaries

`process` validates block shape, all parameter bounds and every input sample
before mutating state or output. Invalid/nonfinite inputs return a typed
error; the future graph must propagate it. This private boundary does not
define public WebIDL error precedence. Infinities/NaNs are not silently
replaced with zero.

Calculations use f64 internally, PCM remains f32. Finite f32 overflow during
makeup/transients saturates only at `±f32::MAX`; no device-style clipping is
applied. Parameter snapshots are copied once per call. The future renderer
owns sample-clock automation and must change snapshots at quantum boundaries.

The written tests cover integer/fractional delay and tail, ratio-one identity,
an independent hard-knee gain/meter oracle, knee continuity/slope, envelope
timing at two rates, parameter changes without clearing state, stereo linking,
input summation versus incorrect per-source processing, chunk persistence,
finite extremes, error atomicity, per-node isolation and storage bounds.
No Rust tests or audio have been executed for this source yet.
