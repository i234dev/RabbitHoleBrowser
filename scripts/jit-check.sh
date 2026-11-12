#!/bin/bash
# Run test suites with the baseline JavaScript compiler off and with every
# function compiled at its first run (threshold 0), and compare the sets
# of passing tests (docs/jit-design.md, section 7).
#
#   scripts/jit-check.sh [cargo test package args...]
#
# Default packages: rabbithole-js, rabbithole-script and the frameworks
# test of rabbithole-agent. Extra environment (e.g. RH_GC_STRESS=1) is
# passed through to both runs.
set -u
cd "$(dirname "$0")/.." || exit 1
if [ $# -eq 0 ]; then
  set -- -p rabbithole-js -p rabbithole-script -p rabbithole-agent
fi
out=${JIT_CHECK_OUT:-$(mktemp -d)}
run() {
  local mode=$1
  shift
  env "$@" cargo test --release --features rabbithole-js/jit "${ARGS[@]}" 2>&1 |
    tee "$out/$mode.log" | grep -E "^test .* \.\.\. (ok|FAILED|ignored)$" | sort > "$out/$mode.txt"
}
ARGS=("$@")
run off RABBITHOLE_JIT=off
run on RABBITHOLE_JIT=on RABBITHOLE_JIT_THRESHOLD=0
echo "off: $(grep -c ' ok$' "$out/off.txt") passed, $(grep -c 'FAILED$' "$out/off.txt") failed"
echo "on:  $(grep -c ' ok$' "$out/on.txt") passed, $(grep -c 'FAILED$' "$out/on.txt") failed"
if diff "$out/off.txt" "$out/on.txt" > "$out/diff.txt"; then
  echo "identical results ($out)"
else
  echo "DIFFERENT results ($out/diff.txt):"
  cat "$out/diff.txt"
  exit 1
fi
