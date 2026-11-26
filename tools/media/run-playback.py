#!/usr/bin/env python3
"""Run shot against each generated clip using only an ephemeral loopback server."""
import argparse
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
import subprocess
import sys
import threading

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("shot", type=Path)
parser.add_argument("corpus", type=Path)
parser.add_argument("output", type=Path)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
handler = partial(SimpleHTTPRequestHandler, directory=str(args.corpus))
env = dict(os.environ, RH_MEDIA_STATS="1", RH_AUDIO="null", RH_JS_WAIT="6000",
           RH_EVAL='[video.readyState,video.currentTime,video.ended,video.videoWidth].join("|")')
with ThreadingHTTPServer(("127.0.0.1", 0), handler) as server:
    threading.Thread(target=server.serve_forever, daemon=True).start()
    for page in sorted(args.corpus.glob("*.html")):
        url = f"http://127.0.0.1:{server.server_port}/{page.name}"
        log = args.output / (page.stem + ".log")
        print(page.stem, flush=True)
        with log.open("w") as stream:
            subprocess.run([args.shot.resolve(), url, args.output / (page.stem + ".png"), "960", "540"],
                           env=env, stderr=stream, stdout=stream, timeout=45, check=True)
        with log.open() as stream:
            result = subprocess.run([sys.executable, Path(__file__).with_name("summarize-playback.py")], stdin=stream)
        if result.returncode:
            print(f"See {log} for codec availability and playback status", flush=True)
