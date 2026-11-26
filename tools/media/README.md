# Offline MSE playback measurement

Generate the six 3-second, 30 fps, video-only testsrc2 clips outside the tree.
Use an optimized build of the engine's `shot` example. The runner serves the
clips on loopback, plays them through MediaSource, paints at each update, and
records the final image and stderr. It needs no external network connection.

```sh
tools/media/generate-playback.sh /tmp/media-corpus
python3 tools/media/run-playback.py /path/to/shot /tmp/media-corpus /tmp/media-results
```

The source resolutions are 1280x720 and 1920x1080. The video occupies 640x360
CSS pixels in a 960x540 viewport. H.264 pictures require Windows Media
Foundation. A run with no painted picture prints a diagnostic.

Set `RH_MEDIA_STATS=1` to collect the same trace in the browser. For one
complete playback, run `python3 tools/media/summarize-playback.py < trace.log`.
Frame ids join decoder output to the content raster. Decode is wall time in
the decoder excluding color conversion. Calls producing no picture accrue to
the next batch. Paint includes incremental presentation and rasterization in
shot, or content worker frame production in the GUI. Latency starts at the
decoder call that yielded the picture and includes queueing. Display refresh,
window composition and physical scanout are outside these measurements.

FPS uses intervals between the first and last painted picture. The difference
between decoded and painted counts includes superseded pictures and pictures
still pending at the end of a truncated trace. Compare completed playbacks at
the same resolution, viewport and build profile. The summary reports means
and nearest-rank p95 values in milliseconds.

## Windows codec selection

With an available H.264 Media Foundation transform, RabbitHole advertises AVC
and suppresses VP9/AV1 and VP8 in explicit codec queries. If the transform is
unavailable, it advertises the previous alternatives and suppresses AVC. To
restore all available alternatives, set `RH_VIDEO_CODECS=all` before starting
the browser. The setting applies to the whole process. Driver acceleration
depends on the Media Foundation configuration and the system.

Close RabbitHole, open the candidate executable, and play a YouTube video
available at 720p and 1080p. In “Stats for nerds”, record the codec, current
resolution, and the change in dropped/total frames over 30 seconds at each
resolution. Check sound, seeking and A/V sync. Repeat the same passage after
starting a new process with `$env:RH_VIDEO_CODECS = 'all'` in PowerShell.
Use Task Manager's GPU Video Decode graph to check hardware use. Remove the
override with `Remove-Item Env:RH_VIDEO_CODECS` before the next default run.
