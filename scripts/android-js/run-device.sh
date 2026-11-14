#!/system/bin/sh
# Run manually on an arm64 Android device after unpacking the bundle.
set -eu
cd "$(dirname "$0")"
command -v timeout >/dev/null || { echo 'A timeout utility is required' >&2; exit 2; }
command -v cmp >/dev/null || { echo 'A cmp utility is required' >&2; exit 2; }
api=$(getprop ro.build.version.sdk)
case "$api" in ''|*[!0-9]*) echo 'Cannot determine Android API level' >&2; exit 2;; esac
[ "$api" -ge 21 ] || { echo 'Android API21 or newer is required' >&2; exit 2; }
case "$(getprop ro.product.cpu.abilist)" in *arm64-v8a*) ;; *) echo 'arm64-v8a is required' >&2; exit 2;; esac
mkdir -p results
{
  printf 'api=%s\n' "$api"
  printf 'release=%s\n' "$(getprop ro.build.version.release)"
  printf 'abis=%s\n' "$(getprop ro.product.cpu.abilist)"
  printf 'machine=%s\n' "$(uname -m)"
} > results/device.txt
failures=0
run_case() {
  name=$1
  fixture=$2
  expected=$3
  marker=$4
  if (
    unset RH_GC_STRESS RH_GC_STATS RH_GC_TRACE RH_NO_GC RH_GC_INCREMENTAL RH_GC_SLICE_US RH_PROF
    export RABBITHOLE_JIT=off
    if [ "$name" = smoke-inc-gc ]; then export RH_GC_STRESS=inc RH_GC_INCREMENTAL=1 RH_GC_STATS=1; fi
    timeout 120 ./rhjs "$fixture"
  ) >"results/$name.stdout" 2>"results/$name.stderr"; then status=0; else status=$?; fi
  printf '%s\n' "$status" >"results/$name.exit"
  if [ "$status" -ne "$expected" ]; then
    echo "FAIL $name: exit $status, expected $expected"
    failures=$((failures + 1))
  elif [ "$expected" -eq 0 ] && ! cmp -s expected.stdout "results/$name.stdout"; then
    echo "FAIL $name: stdout differs from oracle"
    failures=$((failures + 1))
  elif [ "$expected" -ne 0 ] && ! grep -F "$marker" "results/$name.stderr" >/dev/null; then
    echo "FAIL $name: diagnostic marker missing"
    failures=$((failures + 1))
  else
    echo "PASS $name"
  fi
}
run_case smoke smoke.js 0 ''
run_case smoke-inc-gc smoke.js 0 ''
run_case negative-uncaught negative-uncaught.js 1 ANDROID_JS_NEGATIVE_CONTROL
run_case negative-rejection negative-rejection.js 1 ANDROID_JS_REJECTION_CONTROL
printf 'failures=%s\n' "$failures" > results/summary.txt
[ "$failures" -eq 0 ]
