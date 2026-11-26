#!/usr/bin/env python3
"""Read one complete RH_MEDIA_STATS playback trace from stdin (one video)."""
import math
import statistics
import sys

frames = {}
paints = {}
for line in sys.stdin:
    fields = line.split()
    if fields[:2] == ["rh_video", "decode"]:
        _, _, frame, codec, pts, decode, yuv = fields
        frames[frame] = (float(decode), float(yuv))
    elif fields[:2] == ["rh_video", "paint"]:
        _, _, frame, paint, latency, time = fields
        paints[frame] = (float(paint), float(latency), float(time))

if not frames or not paints:
    sys.exit("No frames reached the content raster")
times = [t for _, _, t in paints.values()]
span = max(times) - min(times)
fps = (len(times) - 1) * 1000 / span if span else 0
print(f"decoded={len(frames)} painted={len(paints)} unpainted={len(frames.keys() - paints.keys())} fps={fps:.2f}")
for label, values in (("decode", [d for d, _ in frames.values()]),
                      ("yuv", [y for _, y in frames.values()]),
                      ("paint", [p for p, _, _ in paints.values()]),
                      ("latency", [t for _, t, _ in paints.values()])):
    p95 = sorted(values)[math.ceil(.95 * len(values)) - 1]
    print(f"{label}_ms mean={statistics.mean(values):.3f} p95={p95:.3f}")
