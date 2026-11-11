#!/usr/bin/env bash
# Smoke-test RabbitHole against a fixed list of real sites: element count,
# visible text length, images, and JS errors / script timeouts per site.
# usage: scripts/site-smoke.sh [--wait MS] [url...]
set -u
BIN="${RH_BIN:-target/release/rabbithole}"
WAIT=6000
if [ "${1:-}" = "--wait" ]; then WAIT="$2"; shift 2; fi
SITES=("$@")
if [ ${#SITES[@]} -eq 0 ]; then
  SITES=(
    https://www.youtube.com/
    https://koma.cards/
    https://www.google.com/
    https://en.wikipedia.org/wiki/Rabbit
    https://www.reddit.com/
    https://www.bbc.com/news
    https://news.ycombinator.com/
    https://github.com/rust-lang/rust
    http://acid2.acidtests.org/#top
    http://acid3.acidtests.org/
  )
fi
EXPR="JSON.stringify([document.querySelectorAll('*').length, (document.body&&document.body.innerText||'').length, document.images.length])"
printf '%-42s %8s %8s %6s %7s %8s %7s\n' SITE ELEMENTS TEXT IMGS ERRORS TIMEOUTS SECS
for url in "${SITES[@]}"; do
  out=$(mktemp)
  start=$(date +%s.%N)
  timeout 120 "$BIN" js "$url" "$EXPR" --wait "$WAIT" --verbose >"$out" 2>&1
  secs=$(echo "$(date +%s.%N) - $start" | bc)
  res=$(grep -o '\[[0-9]*,[0-9]*,[0-9]*\]' "$out" | tail -1 | tr -d '[]' | tr ',' ' ')
  read -r els txt imgs <<<"${res:-- - -}"
  errs=$(grep -ciE 'uncaught|error' "$out")
  tos=$(grep -c 'Script execution timed out' "$out")
  printf '%-42s %8s %8s %6s %7s %8s %7.1f\n' "${url:0:42}" "$els" "$txt" "$imgs" "$errs" "$tos" "$secs"
  rm -f "$out"
done
