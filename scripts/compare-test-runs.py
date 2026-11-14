#!/usr/bin/env python3
"""Compare libtest outcomes after both Cargo commands have succeeded.

The caller must preserve each Cargo exit status: matching failed builds or empty
logs are not evidence of interpreter/JIT parity. Keep duplicate test names; they
can occur in distinct test binaries or documentation test groups.
"""
from collections import Counter
from pathlib import Path
import re
import sys


def outcomes(path):
    result = Counter()
    for line in Path(path).read_text(encoding="utf-8", errors="replace").splitlines():
        match = re.fullmatch(r"test (.+) \.\.\. (ok|FAILED|ignored)(?:, .*)?", line)
        if match:
            result[(match[1], match[2])] += 1
    if not result:
        raise ValueError(f"{path}: no test outcomes recorded")
    if any(status == "FAILED" for _, status in result):
        raise ValueError(f"{path}: contains failed tests")
    return result


def main():
    if len(sys.argv) != 3:
        raise ValueError("usage: compare-test-runs.py INTERPRETER_LOG JIT_LOG")
    off, on = (outcomes(path) for path in sys.argv[1:])
    if off != on:
        for label, difference in (("interpreter only", off - on), ("JIT only", on - off)):
            for (name, status), count in sorted(difference.items()):
                print(f"{label}: {count} x {name} ... {status}", file=sys.stderr)
        raise ValueError("executed test outcomes differ")
    passed = sum(count for (_, status), count in off.items() if status == "ok")
    ignored = sum(count for (_, status), count in off.items() if status == "ignored")
    print(f"Identical outcomes: {passed} passed, {ignored} ignored in each mode")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        sys.exit(str(error))
